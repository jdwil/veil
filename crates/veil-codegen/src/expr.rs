//! Expression translator — converts VEIL AST Expr to Rust source code.
//!
//! Fully shape-driven: the translator uses `GenCtx.name_to_shape` to decide
//! how to emit a Call (port call → deps.x.method().await?, struct call →
//! Type::new(args), local → target.method(args)).

use std::collections::{HashMap, HashSet};

use veil_ir::ast::*;
use veil_ir::layer::{Shape, StmtShape, LayerRegistry};

use crate::rust::to_snake;

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
    /// Whether cross-boundary calls use message-envelope routing (JSON) via
    /// layer-declared routing traits. Opt-in when loaded layers declare statement
    /// targets that are routing ports (INV-003).
    pub envelope_routing: bool,
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
    /// Expression that names the primary routing-trait instance for envelope
    /// routing. Derived from layer routing traits: `deps.<snake(Trait)>` in a
    /// flow; the injected param name inside a runtime-delegated step method.
    /// Empty when no routing traits are loaded.
    pub routing_ref: String,
    /// Names of traits used as message-routing ports (from layer statement
    /// `maps_to Trait.method`). Calls to these use `routing_ref` instead of
    /// `deps.<name>`.
    pub routing_traits: HashSet<String>,
    /// Names of known async free functions (layer-declared coordinators and
    /// package free fns). Calls to these need `.await?`.
    pub async_fns: HashSet<String>,
    /// Names backed by a threaded JSON state bag (multi-step runtime-delegated
    /// constructs). A read of such a name becomes `state["name"]`; an assignment
    /// writes `state["name"] = ...` so step impls can share results.
    pub state_locals: HashSet<String>,
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
    /// Locals whose first binding must be `let mut` (reassigned, field-written,
    /// or receiver of a known mutating method). Plain `Assign` without this
    /// set emits immutable `let`. Explicit `mut x = …` always uses `let mut`.
    pub mut_locals: HashSet<String>,
    /// Stub package free-fn roots: use-alias / crate name → rust crate ident.
    /// e.g. `crypto` / `relay_crypto` / `relay-crypto` → `relay_crypto`.
    pub stub_pkg_crate: HashMap<String, String>,
    /// Stub free functions: (rust_crate, fn_name_without_bang) → fallible (Res!).
    pub stub_free_fns: HashMap<(String, String), bool>,
    /// Bus message name → Rust success type for `invoke`/`request` decode
    /// (e.g. `"Reconcile"` → `"ReconcileResult"`). Json/unit stay as Value.
    pub bus_returns: HashMap<String, String>,
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
}

impl GenCtx {
    pub fn new(name_to_shape: HashMap<String, Shape>) -> Self {
        GenCtx {
            name_to_shape,
            locals: HashSet::new(),
            self_fields: HashSet::new(),
            in_method: false,
            envelope_routing: false,
            method_returns: HashMap::new(),
            method_params: HashMap::new(),
            ref_params: HashMap::new(),
            local_types: HashMap::new(),
            struct_fields: HashMap::new(),
            routing_ref: String::new(),
            routing_traits: HashSet::new(),
            async_fns: HashSet::new(),
            state_locals: HashSet::new(),
            stub_type_crate: HashMap::new(),
            stub_typed_ctors: HashMap::new(),
            fallible_methods: HashSet::new(),
            non_fallible_methods: HashSet::new(),
            type_fallible_methods: HashSet::new(),
            async_fallible_methods: HashSet::new(),
            expected_return_rust: None,
            option_value_wrap: false,
            defaultable_types: HashSet::new(),
            dep_fields: HashMap::new(),
            mut_locals: HashSet::new(),
            stub_pkg_crate: HashMap::new(),
            stub_free_fns: HashMap::new(),
            bus_returns: HashMap::new(),
            local_domain_types: HashSet::new(),
            self_field_types: HashMap::new(),
            statement_specs: HashMap::new(),
            enum_variants: HashMap::new(),
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
        let mut names: Vec<&str> = self.routing_traits.iter().map(|s| s.as_str()).collect();
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
        self.local_types.get(name).map(|s| s.as_str())
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
            if let Some(t) = self.method_returns.get(&(target.to_string(), m.clone())) {
                return Some(t.as_str());
            }
        }
        // If target is a local, look up its type and check struct methods
        if let Some(type_name) = self.local_types.get(target) {
            for m in &keys {
                if let Some(t) = self
                    .method_returns
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
        self.struct_fields.get(type_name)
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
            if let Some(params) = self.method_params.get(key) {
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
        if c.shape == Shape::Fn {
            if let Some(rt) = &c.return_type {
                let rust = crate::rust::type_to_rust(rt);
                // Strip outer Result if present (VEIL return is the success type).
                let inner = rust
                    .strip_prefix("Result<")
                    .and_then(|s| s.rsplit_once(", "))
                    .map(|(a, _)| a.trim().to_string())
                    .unwrap_or(rust);
                if inner != "()" && !inner.is_empty() {
                    let msg = registry.bus_message_name(&c.name);
                    ctx.bus_returns.insert(msg, inner.clone());
                    ctx.bus_returns.insert(c.name.clone(), inner);
                }
            }
        }
        // Record method return types for trait-shaped constructs
        if c.shape == Shape::Trait {
            for method in &c.methods {
                let ret_type = method.return_type.as_ref()
                    .map(|t| extract_inner_type(t))
                    .unwrap_or_else(|| "()".to_string());
                let bare_method = method.name.trim_end_matches(['!', '?']).to_string();
                // Res! / Result, or a bang on the signature, is fallible.
                // Unit methods without bang must not get `.await?`.
                let is_result = matches!(method.return_type, Some(TypeExpr::Result(_)));
                if is_result || method.name.ends_with('!') {
                    ctx.type_fallible_methods
                        .insert((c.name.clone(), bare_method.clone()));
                    ctx.type_fallible_methods
                        .insert((to_snake(&c.name), bare_method.clone()));
                    ctx.type_fallible_methods
                        .insert((c.name.clone(), method.name.clone()));
                }
                // Register under PascalCase trait name (e.g. "CohortRepo", "find")
                ctx.method_returns.insert(
                    (c.name.clone(), method.name.clone()),
                    ret_type.clone(),
                );
                ctx.method_returns
                    .insert((c.name.clone(), bare_method.clone()), ret_type.clone());
                // Also register under snake_case dep name (e.g. "cohort_repo", "find")
                // so lookups from @dep variable names resolve without conversion
                ctx.method_returns.insert(
                    (to_snake(&c.name), method.name.clone()),
                    ret_type.clone(),
                );
                ctx.method_returns
                    .insert((to_snake(&c.name), bare_method.clone()), ret_type.clone());
                // Record parameter types for each method so call-site arg coercion
                // can check whether a port expects Option<T> vs T.
                let param_types: Vec<String> = method.params.iter()
                    .map(|p| type_name_simple(&p.type_expr))
                    .collect();
                ctx.method_params.insert(
                    (c.name.clone(), method.name.clone()),
                    param_types.clone(),
                );
                ctx.method_params.insert(
                    (to_snake(&c.name), method.name.clone()),
                    param_types.clone(),
                );
                ctx.method_params.insert(
                    (c.name.clone(), bare_method.clone()),
                    param_types.clone(),
                );
                ctx.method_params.insert(
                    (to_snake(&c.name), bare_method),
                    param_types,
                );
                // Type aliases (WearTestRepo = EntityRepo<WearTest>) share methods —
                // also register under any alias that monomorphizes this trait.
            }
        }

        // Bare enum variants (`Healthy`) → `DaemonStatus::Healthy`.
        if c.shape == Shape::Enum {
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
            ctx.struct_fields.insert(c.name.clone(), fields);

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
            ctx.method_returns.insert(
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
                    .method_returns
                    .keys()
                    .filter(|(t, _)| t == base || t == &to_snake(base))
                    .cloned()
                    .collect();
                for (t, method) in base_keys {
                    if let Some(ret) = ctx.method_returns.get(&(t, method.clone())).cloned() {
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
                        ctx.method_returns
                            .insert((name.clone(), method.clone()), mono.clone());
                        ctx.method_returns
                            .insert((to_snake(name), method), mono);
                    }
                }
            } else if let TypeExpr::Named(base) = target {
                ctx.name_to_shape.insert(name.clone(), Shape::Trait);
                let base_keys: Vec<_> = ctx
                    .method_returns
                    .keys()
                    .filter(|(t, _)| t == base || t == &to_snake(base))
                    .cloned()
                    .collect();
                for (t, method) in base_keys {
                    if let Some(ret) = ctx.method_returns.get(&(t, method.clone())).cloned() {
                        ctx.method_returns
                            .insert((name.clone(), method.clone()), ret.clone());
                        ctx.method_returns
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
        ctx.stub_pkg_crate
            .insert(stub.name.clone(), rust_crate.clone());
        ctx.stub_pkg_crate
            .insert(rust_crate.clone(), rust_crate.clone());
        if let Some(alias) = &stub.alias {
            ctx.stub_pkg_crate
                .insert(alias.clone(), rust_crate.clone());
        }
        for ff in &stub.free_fns {
            let bare = ff.name.trim_end_matches(['!', '?']).to_string();
            let ret = ff.return_type.as_deref().unwrap_or("()");
            let fallible = ret.starts_with("Res!") || ret.starts_with("Res!<") || ret.contains("Res!");
            if fallible {
                ctx.fallible_methods.insert(ff.name.clone());
                ctx.fallible_methods.insert(bare.clone());
            } else {
                ctx.non_fallible_methods.insert(ff.name.clone());
                ctx.non_fallible_methods.insert(bare.clone());
            }
            ctx.stub_free_fns
                .insert((rust_crate.clone(), bare.clone()), fallible);
            // Register return type for type inference (crate name acts as "type"):
            let inner = if ret.starts_with("Res!<") {
                ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
            } else if ret == "Res!" {
                "()"
            } else {
                ret
            };
            ctx.method_returns.insert(
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
                    }));
                if fallible {
                    ctx.fallible_methods.insert(method.name.clone());
                    ctx.type_fallible_methods.insert((type_name.clone(), method.name.clone()));
                } else {
                    ctx.non_fallible_methods.insert(method.name.clone());
                }
                if is_async_fallible {
                    ctx.async_fallible_methods.insert(method.name.clone());
                }
                let inner = if ret.starts_with("Res!<") {
                    ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
                } else if ret == "Res!" {
                    "()"
                } else {
                    ret
                };
                ctx.method_returns.insert(
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
                ctx.method_params
                    .insert((type_name.clone(), method.name.clone()), param_types);
                // Track ref-pass parameters for this method
                let has_any_ref = method.params.iter().any(|p| p.2);
                if has_any_ref {
                    let ref_flags: Vec<bool> = method.params.iter().map(|p| p.2).collect();
                    ctx.ref_params.insert(
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
            ctx.stub_type_crate
                .insert(type_name.clone(), (crate_name.clone(), path_type.clone()));
            // Crate-qualified keys so `aws_sdk_sns.Client` is not confused with
            // `aws_sdk_dynamodb.Client`.
            ctx.stub_type_crate.insert(
                format!("{crate_name}.{}", s.name),
                (crate_name.clone(), path_type.clone()),
            );
            ctx.stub_type_crate.insert(
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
                ctx.stub_type_crate
                    .entry(s.name.clone())
                    .or_insert_with(|| (crate_name.clone(), path_type.clone()));
            }
            // Also register under the bare (unaliased) name so VEIL source can use
            // `AttributeValue.S(pk)` even when the stub is aliased (e.g. `use ... as ddb`).
            if stub.alias.is_some() && s.name != type_name && bare_hits <= 1 {
                ctx.name_to_shape.entry(s.name.clone()).or_insert(Shape::Struct);
                ctx.stub_type_crate
                    .entry(s.name.clone())
                    .or_insert_with(|| (crate_name, path_type));
            }
            // Typed free-fn constructor (e.g. Query → query_as) from stub metadata.
            if let Some(ref typed_fn) = s.typed_variant {
                let params = s
                    .typed_type_params
                    .clone()
                    .unwrap_or_else(|| "_, return_type".into());
                ctx.stub_typed_ctors
                    .insert(type_name, (typed_fn.clone(), params.clone()));
                // Also key by bare stub type name (unaliased)
                ctx.stub_typed_ctors
                    .insert(s.name.clone(), (typed_fn.clone(), params));
            }
        }
        for i in &stub.impls {
            for method in &i.methods {
                let ret = method.return_type.as_deref().unwrap_or("()");
                let fallible = ret.starts_with("Res!") || ret.starts_with("Res!<");
                if fallible {
                    ctx.fallible_methods.insert(method.name.clone());
                    ctx.type_fallible_methods.insert((i.target.clone(), method.name.clone()));
                } else {
                    ctx.non_fallible_methods.insert(method.name.clone());
                }
                let inner = if ret.starts_with("Res!<") {
                    ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
                } else if ret == "Res!" {
                    "()"
                } else {
                    ret
                };
                ctx.method_returns.insert(
                    (i.target.clone(), method.name.clone()),
                    inner.to_string(),
                );
            }
        }
    }

    // Populate routing traits from the layer registry so call generation can
    // identify message-routing ports without hardcoding trait names.
    ctx.routing_traits = registry.routing_traits().into_iter().collect();
    ctx.routing_ref = ctx.default_routing_ref_as_dep();

    // Layer statement specs for custom `lowers_to` template emission.
    for stmt in &registry.statements {
        ctx.statement_specs.insert(stmt.keyword.clone(), stmt.clone());
    }

    // Track layer-declared free functions as async — they generate as
    // `pub async fn` and calls to them need `.await?`. Product free fns are
    // emitted in the product crate (sync unless they call async helpers).
    for item in &solution.items {
        if let TopLevelItem::Function(f) = item {
            if f.layer_provided {
                ctx.async_fns.insert(f.name.clone());
            }
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
        let names: Vec<String> = ctx.struct_fields.keys().cloned().collect();
        for type_name in names {
            if ctx.defaultable_types.contains(&type_name) {
                continue;
            }
            let fields = ctx.struct_fields.get(&type_name).cloned().unwrap_or_default();
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

/// Whether a stored struct field (Rust type string) can be filled without a
/// `new(...)` argument. Mirrors smart-ctor defaults in rust.rs.
fn rust_field_is_defaultable(
    field_name: &str,
    rust_ty: &str,
    ctor_pol: &veil_ir::layer::ConstructorPolicy,
    defaultable: &HashSet<String>,
) -> bool {
    if ctor_pol.is_auto_field(field_name) {
        return true;
    }
    // Field-name string defaults (constructor_policy-adjacent conventions).
    if field_name == "authorization_header_string" {
        return true;
    }
    let t = rust_ty.trim();
    if t.starts_with("Option<")
        || t.starts_with("Vec<")
        || t.contains("HashMap")
        || t.contains("HashSet")
    {
        return true;
    }
    // INV-002 scalar type defaults (policy table, not domain words).
    for (veil_ty, _) in &ctor_pol.type_defaults {
        let rust = rust_type_for_named(veil_ty);
        if t == rust {
            return true;
        }
    }
    // Nested type already known defaultable.
    if defaultable.contains(t) {
        return true;
    }
    // Unit enums and domain types that implement Default appear as bare names.
    // Only treat as defaultable once registered (fixpoint / unit-enum pass).
    false
}

/// Record a unit variant → enum type. Ambiguous names (two enums, same variant)
/// are dropped so we never invent the wrong qualifier.
fn register_enum_variant(ctx: &mut GenCtx, variant: &str, enum_name: &str) {
    if variant.is_empty()
        || matches!(
            variant,
            "Ok" | "Err" | "Some" | "None" | "true" | "false" | "_" | "null" | "noop"
        )
    {
        return;
    }
    match ctx.enum_variants.get(variant) {
        Some(existing) if existing != enum_name => {
            ctx.enum_variants.remove(variant);
        }
        Some(_) => {}
        None => {
            ctx.enum_variants
                .insert(variant.to_string(), enum_name.to_string());
        }
    }
}

/// Qualified constructor for a stub type (`example_sdk::primitives::Blob`).
/// Accepts a bare name (`Blob`) or a crate-qualified VEIL path
/// (`aws_sdk_lambda.Blob` / `aws_sdk_lambda::Blob`). Falls back to the
/// leaf name so rustc names the missing type instead of emitting `Vec<u8>`.
fn stub_ctor_path(ctx: &GenCtx, type_name: &str) -> String {
    if let Some((c, p)) = stub_type_parts(ctx, type_name) {
        return format!("{c}::{p}");
    }
    lang_type_leaf(type_name).to_string()
}

/// `(crate, rust_type_path)` for a stub type. Tries the written name, then
/// `crate.Leaf` / `crate::Leaf`, then a unique bare leaf. Never invents a
/// module — `rust_type_path` on the stub is the only source of `types::` /
/// `primitives::`.
fn stub_type_parts<'a>(ctx: &'a GenCtx, type_name: &str) -> Option<(&'a str, &'a str)> {
    if let Some((c, p)) = ctx.stub_type_crate.get(type_name) {
        return Some((c.as_str(), p.as_str()));
    }
    let leaf = lang_type_leaf(type_name);
    let crate_guess = type_name
        .split(['.', ':'])
        .next()
        .unwrap_or("")
        .replace('-', "_");
    if !crate_guess.is_empty() && leaf != type_name {
        for key in [
            format!("{crate_guess}.{leaf}"),
            format!("{crate_guess}::{leaf}"),
        ] {
            if let Some((c, p)) = ctx.stub_type_crate.get(&key) {
                return Some((c.as_str(), p.as_str()));
            }
        }
    }
    if leaf != type_name {
        if let Some((c, p)) = ctx.stub_type_crate.get(leaf) {
            return Some((c.as_str(), p.as_str()));
        }
    }
    None
}

/// Last path segment of a VEIL type (`aws_sdk_lambda.Blob` → `Blob`).
fn lang_type_leaf(target: &str) -> &str {
    target
        .rsplit(['.', ':'])
        .find(|s| !s.is_empty())
        .unwrap_or(target)
}

fn method_bare(method: &str) -> &str {
    method.trim_end_matches(['!', '?'])
}

/// SDK / stub `Res!` errors are often `&T` or types with Debug but not Display.
/// Never use `e.to_string()` for unknown E.
fn map_err_domain() -> &'static str {
    r#".map_err(|e| DomainError::External(format!("{e:?}")))?"#
}

/// `Res!<Str>` on the Rust side is usually `Result<&str, E>`. VEIL `Str` is
/// owned `String`, so own the payload and map the error via Debug.
fn map_err_domain_own_str() -> &'static str {
    r#".map(|s| s.to_string()).map_err(|e| DomainError::External(format!("{e:?}")))?"#
}

fn is_str_like_return(ty: &str) -> bool {
    let t = ty.trim();
    matches!(
        t,
        "Str" | "String" | "&str" | "&String" | "Res!<Str>" | "Opt<Str>"
    ) || t.starts_with("Result<String")
        || t.starts_with("Result<&str")
        || t.starts_with("Result<&String")
}

/// True when a stub method's success type is VEIL `Str` (own a `String`).
/// Name fallback is only `as_s` / `as_n` — other `as_*` extractors return
/// maps, lists, bools, bytes.
fn should_own_str_result(ctx: &GenCtx, recv_ty: Option<&str>, method: &str) -> bool {
    let bare = method_bare(method);
    if let Some(ty) = recv_ty {
        let leaf = lang_type_leaf(ty);
        for key in [ty, leaf] {
            if let Some(ret) = ctx.return_type_of(key, bare) {
                return is_str_like_return(ret);
            }
        }
    }
    matches!(bare, "as_s" | "as_n")
}

fn peel_option_rust(ty: &str) -> Option<&str> {
    let t = ty.trim();
    t.strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .or_else(|| t.strip_prefix("Opt<").and_then(|s| s.strip_suffix('>')))
}

fn rust_ty_is_stringish(ty: &str) -> bool {
    matches!(
        ty.trim(),
        "String" | "Str" | "&str" | "&String" | "&'static str"
    )
}

fn rust_ty_is_numeric(ty: &str) -> bool {
    matches!(
        ty.trim(),
        "i64" | "i32" | "u64" | "u32" | "usize" | "isize" | "f64" | "f32" | "Int" | "F64"
    )
}

fn rust_ty_is_copy(ty: &str) -> bool {
    matches!(
        ty.trim(),
        "i64" | "i32" | "i16" | "i8"
            | "u64" | "u32" | "u16" | "u8"
            | "usize" | "isize"
            | "f64" | "f32"
            | "bool"
            | "Int" | "F64" | "Bool"
    )
}

fn rust_success_is_str(ty: &str) -> bool {
    if is_str_like_return(ty) {
        return true;
    }
    let t = ty.trim();
    if let Some(inner) = t.strip_prefix("Result<") {
        let success = inner
            .rsplit_once(',')
            .map(|(a, _)| a.trim())
            .unwrap_or(inner);
        return is_str_like_return(success)
            || success == "Option<String>"
            || success == "Option<Str>";
    }
    false
}

fn rust_ty_is_option_or_result(ty: &str) -> bool {
    let t = ty.trim();
    t.starts_with("Option<")
        || t.starts_with("Opt<")
        || t.starts_with("Result<")
        || t.starts_with("Res!")
}

fn rust_ty_is_bytes_like(ty: &str) -> bool {
    let leaf = lang_type_leaf(ty);
    leaf == "Blob"
        || leaf == "Bytes"
        || ty == "Vec<u8>"
        || ty.ends_with("::Blob")
        || ty.contains("Blob")
}

/// VEIL values are reusable. A field read is a copy of the field, not a move.
fn field_access_is_copy(base: &Expr, field: &str, ctx: &GenCtx) -> bool {
    let base_ty = match base {
        Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
        _ => infer_expr_type(base, ctx),
    };
    let Some(base_ty) = base_ty else {
        return false;
    };
    let peeled = peel_option_rust(&base_ty).unwrap_or(base_ty.as_str());
    let leaf = lang_type_leaf(peeled);
    for key in [peeled, leaf] {
        if let Some(ft) = ctx
            .field_type(key, field)
            .or_else(|| ctx.field_type(key, &to_snake(field)))
        {
            return rust_ty_is_copy(ft);
        }
    }
    false
}

fn recv_rust_type(recv: &Expr, ctx: &GenCtx) -> Option<String> {
    match recv {
        Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
        _ => infer_expr_type(recv, ctx),
    }
}

/// `as_ref` / bytes view used where VEIL wants `Str` → utf-8 decode.
/// Never rewrite `Option`/`Result`/`String` `.as_ref()`.
fn should_decode_as_ref_to_str(recv: &Expr, ctx: &GenCtx) -> bool {
    let recv_ty = recv_rust_type(recv, ctx);
    if let Some(ty) = recv_ty.as_deref() {
        if rust_ty_is_option_or_result(ty) || rust_ty_is_stringish(ty) {
            return false;
        }
        if should_own_str_result(ctx, Some(ty), "as_ref") || rust_ty_is_bytes_like(ty) {
            return true;
        }
    }
    if ctx
        .expected_return_rust
        .as_deref()
        .is_some_and(rust_success_is_str)
    {
        return true;
    }
    ctx.method_returns.iter().any(|((ty, method), ret)| {
        method_bare(method) == "as_ref"
            && is_str_like_return(ret)
            && !rust_ty_is_option_or_result(ty)
    })
}

fn now_iso8601_rust() -> String {
    "Utc::now().to_rfc3339()".to_string()
}

fn expr_is_stringish(expr: &Expr, rust: &str, ctx: &GenCtx) -> bool {
    match expr {
        Expr::StringLit(_) => true,
        Expr::Ident(n) => ctx.local_type(n).is_some_and(rust_ty_is_stringish),
        _ => infer_expr_type(expr, ctx)
            .as_deref()
            .is_some_and(rust_ty_is_stringish)
            || rust.contains(".to_string()")
            || rust.trim_start().starts_with('"'),
    }
}

fn expr_is_numeric(expr: &Expr, ctx: &GenCtx) -> bool {
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) => true,
        Expr::Ident(n) => ctx.local_type(n).is_some_and(rust_ty_is_numeric),
        _ => infer_expr_type(expr, ctx)
            .as_deref()
            .is_some_and(rust_ty_is_numeric),
    }
}

/// `format!("{}{}", ident, field)` must not move locals reused later.
fn clone_if_named_value(expr: &Expr, rust: String) -> String {
    if rust.ends_with(".clone()") || rust.starts_with('"') || rust.starts_with("format!(") {
        return rust;
    }
    match expr {
        Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("{rust}.clone()"),
        _ => rust,
    }
}

/// Drop a trailing try-suffix so a `match` can consume a `Result` directly.
/// String-pattern matches must **not** use this — they need the unwrapped value.
fn strip_try_suffix(raw: String) -> String {
    raw.strip_suffix(".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?")
        .or_else(|| {
            raw.strip_suffix(".await.map_err(|e| DomainError::External(format!(\"{:?}\", e)))?")
        })
        .map(|s| format!("{s}.await"))
        .or_else(|| {
            raw.strip_suffix(".await.map_err(|e| DomainError::External(e.to_string()))?")
                .map(|s| format!("{s}.await"))
        })
        .or_else(|| raw.strip_suffix(".await?").map(|s| format!("{s}.await")))
        .or_else(|| {
            raw.strip_suffix(".map(|s| s.to_string()).map_err(|e| DomainError::External(format!(\"{e:?}\")))?")
                .map(|s| s.to_string())
        })
        .or_else(|| {
            raw.strip_suffix(map_err_domain())
                .or_else(|| raw.strip_suffix(".map_err(|e| DomainError::External(e.to_string()))?"))
                .map(|s| s.to_string())
        })
        .or_else(|| raw.strip_suffix('?').map(|s| s.to_string()))
        .unwrap_or(raw)
}

fn expr_handles_option_wrap(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Match(_, _) | Expr::IfExpr(_) | Expr::Return(_) | Expr::IfLet { .. }
    )
}

/// `null` / `()` → `None`; already-Option locals stay as-is; else `Some(val)`.
fn wrap_as_option_value(expr: &Expr, rust: String, ctx: &GenCtx) -> String {
    let t = rust.trim();
    if t == "None" || t == "()" {
        return "None".to_string();
    }
    if t.starts_with("Some(") || t.starts_with("return ") {
        return rust;
    }
    if let Expr::Ident(n) = expr {
        if ctx
            .local_type(n)
            .is_some_and(|ty| ty.starts_with("Option<"))
        {
            return rust;
        }
    }
    format!("Some({rust})")
}

/// True when `Type.new` is a module free-fn (`sqlx::query`), not `Type::new`.
/// Stub metadata only — never a type-name special case (`Query` is also a
/// DynamoDB rustdoc type with `fn new()`).
fn stub_new_is_module_free_fn(ctx: &GenCtx, effective_target: &str, type_leaf: &str) -> bool {
    ctx.stub_typed_ctors.contains_key(effective_target)
        || ctx.stub_typed_ctors.contains_key(type_leaf)
        || ctx
            .stub_type_crate
            .contains_key(&format!("{type_leaf}As"))
        || ctx.name_to_shape.contains_key(&format!("{type_leaf}As"))
}

fn bytes_from_str_expr(arg: &str) -> String {
    format!("{{ let __s = ({arg}).to_string(); __s.into_bytes() }}")
}

fn bytes_from_hex_expr(hex_expr: &str) -> String {
    format!(
        "{{ let __h: String = ({hex_expr}).to_string(); let __h = __h.as_str(); let mut __b = Vec::with_capacity(__h.len() / 2); let mut __i = 0usize; while __i + 1 < __h.len() {{ if let Ok(__v) = u8::from_str_radix(&__h[__i..__i + 2], 16) {{ __b.push(__v); }} __i += 2; }} __b }}"
    )
}

/// Extract the inner type from a TypeExpr (unwrapping Result/Optional).
fn extract_inner_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Result(Some(inner)) => type_name_simple(inner),
        TypeExpr::Result(None) => "()".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_name_simple(inner)),
        _ => type_name_simple(ty),
    }
}

/// Get a simple type name string from a TypeExpr.
/// Extract the inner domain struct type from a return type string.
/// e.g., `Result<Option<Tenant>, DomainError>` → Some("Tenant")
/// e.g., `Result<Vec<Cohort>, DomainError>` → Some("Cohort")
/// Only returns Some when the extracted type is a known struct in name_to_shape
/// AND all its fields are primitive types that a DB row can decode directly.
fn extract_domain_type_from_return(
    ret: &str,
    name_to_shape: &HashMap<String, Shape>,
) -> Option<String> {
    // Strip Result<..., DomainError> wrapper
    let inner = ret
        .strip_prefix("Result<")
        .and_then(|s| s.rsplit_once(", DomainError>"))
        .map(|(inner, _)| inner)
        .unwrap_or(ret);
    // Strip Option<...> / Vec<...>
    let type_name = inner
        .strip_prefix("Option<").and_then(|s| s.strip_suffix('>'))
        .or_else(|| inner.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')))
        .unwrap_or(inner);
    // Check if it's a known struct
    if name_to_shape.get(type_name) == Some(&Shape::Struct) {
        Some(type_name.to_string())
    } else {
        None
    }
}

/// Expand stub `typed_type_params` template (`_, return_type` → `_, CohortDTO`).
fn expand_typed_type_params(template: &str, domain_type: &str) -> String {
    template
        .split(',')
        .map(|p| {
            let t = p.trim();
            if t == "return_type" || t == "$ret" {
                domain_type.to_string()
            } else {
                t.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn type_name_simple(ty: &TypeExpr) -> String {
    match ty {
        // Map VEIL builtins to their Rust form so inferred return types /
        // method_returns can be pasted into signatures (Json → serde_json::Value).
        TypeExpr::Named(n) => rust_type_for_named(n),
        TypeExpr::Generic(n, args) => {
            // Keep domain generics (EntityRepo<T>) by name; map List/Map/etc.
            match n.as_str() {
                "List" | "Vec" if args.len() == 1 => {
                    format!("Vec<{}>", type_name_simple(&args[0]))
                }
                "Opt" | "Option" if args.len() == 1 => {
                    format!("Option<{}>", type_name_simple(&args[0]))
                }
                "Map" | "HashMap" if args.len() == 2 => {
                    format!(
                        "HashMap<{}, {}>",
                        type_name_simple(&args[0]),
                        type_name_simple(&args[1])
                    )
                }
                "Set" | "HashSet" if args.len() == 1 => {
                    format!("HashSet<{}>", type_name_simple(&args[0]))
                }
                _ => n.clone(),
            }
        }
        TypeExpr::Result(Some(inner)) => type_name_simple(inner),
        TypeExpr::Result(None) => "()".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_name_simple(inner)),
        TypeExpr::List(inner) => format!("Vec<{}>", type_name_simple(inner)),
        TypeExpr::Map(k, v) => format!("HashMap<{}, {}>", type_name_simple(k), type_name_simple(v)),
        TypeExpr::Set(inner) => format!("HashSet<{}>", type_name_simple(inner)),
        TypeExpr::Tuple(items) => {
            let parts = items.iter().map(type_name_simple).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        TypeExpr::Array(inner, size) => format!("[{}; {}]", type_name_simple(inner), size),
        TypeExpr::Ref(inner, _) => type_name_simple(inner),
        TypeExpr::Dyn(inner) => format!("dyn {}", type_name_simple(inner)),
        TypeExpr::ImplTrait(inner) => format!("impl {}", type_name_simple(inner)),
        TypeExpr::FnPtr(_, _) => "fn()".to_string(),
        TypeExpr::LitStr(_) => "str".to_string(),
    }
}

/// Translate a VEIL expression to a Rust expression string (no trailing semicolon).
pub fn expr_to_rust(expr: &Expr, ctx: &GenCtx) -> String {
    if ctx.option_value_wrap && !expr_handles_option_wrap(expr) {
        let mut inner_ctx = ctx.clone_for_inference();
        inner_ctx.option_value_wrap = false;
        let inner = expr_to_rust(expr, &inner_ctx);
        return wrap_as_option_value(expr, inner, ctx);
    }
    match expr {
        Expr::Ident(name) => {
            // VEIL null → Rust None
            if name == "null" {
                return "None".to_string();
            }
            // VEIL noop → Rust empty block (no-op)
            if name == "noop" {
                return "{}".to_string();
            }
            // Issue 5: Handle inline ternary with nested f-strings from parse_fstring_parts.
            // These arrive as raw text like: `if x.is_some() then f" in {x.unwrap()}" else ""`
            if name.contains(" then ") && (name.contains("f\"") || name.contains("f'")) {
                return translate_inline_ternary_fstring(name);
            }
            // Raw method call idents from fstring parsing: x.unwrap_or("literal")
            // String literal defaults in unwrap_or need .to_string() for Option<String>.
            if name.contains(".unwrap_or(\"") && name.ends_with("\")") {
                // Transform: x.unwrap_or("text") → x.unwrap_or("text".to_string())
                let converted = name.replace(".unwrap_or(\"", ".unwrap_or(\"")
                    .replacen("\")", "\".to_string())", 1);
                return converted;
            }
            if ctx.state_locals.contains(name.as_str()) {
                // Threaded step state: read from the shared JSON bag.
                format!("state[\"{}\"]", name)
            } else if ctx.in_method && !ctx.locals.contains(name.as_str()) {
                if let Some(rf) = resolve_self_field_name(ctx, name) {
                    if rf == "pool" {
                        "&self.pool".to_string()
                    } else {
                        format!("self.{rf}.clone()")
                    }
                } else if name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    if let Some(enum_ty) = ctx.enum_variants.get(name) {
                        format!("{enum_ty}::{name}")
                    } else {
                        name.clone()
                    }
                } else {
                    name.clone()
                }
            } else if !ctx.locals.contains(name)
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                if let Some(enum_ty) = ctx.enum_variants.get(name) {
                    format!("{enum_ty}::{name}")
                } else {
                    name.clone()
                }
            } else {
                name.clone()
            }
        }
        Expr::FieldAccess(base, field) => {
            // `opt.is_some` (no call) is the same predicate as `opt.is_some()`.
            if field == "is_some" || field == "is_none" {
                return format!("{}.{field}()", expr_to_rust(base, ctx));
            }
            // A field of a state-local: index into the threaded JSON state.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return format!("state[\"{}\"][\"{}\"]", name, field);
                }
                // Method body: `self.table` → clone so `&self` methods compile.
                // `self.pool` stays uncloned — sqlx `Executor` is for `&Pool`.
                if name == "self" && ctx.in_method {
                    let f = resolve_self_field_name(ctx, field).unwrap_or_else(|| to_snake(field));
                    if f == "pool" {
                        return "&self.pool".to_string();
                    }
                    if ctx.self_fields.contains(field.as_str())
                        || ctx.self_fields.contains(&f)
                        || ctx.self_field_types.contains_key(&f)
                    {
                        return format!("self.{}.clone()", f);
                    }
                    return format!("self.{}", f);
                }
                // Enum variant access: EnumName.Variant → EnumName::Variant
                // Keep PascalCase field names (S, Hash, PayPerRequest) as variant ids.
                let field_is_variant = field
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
                if matches!(ctx.name_to_shape.get(name.as_str()), Some(Shape::Enum)) {
                    let variant = if field_is_variant {
                        field.clone()
                    } else {
                        field.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
                            + &field[1..]
                    };
                    return format!("{}::{}", name, variant);
                }
                // Stub enums are registered as Struct shapes; PascalCase field access
                // still means a unit variant (ScalarAttributeType.S, Runtime.Nodejs20x).
                if field_is_variant {
                    if let Some((crate_name, path_type)) = ctx.stub_type_crate.get(name.as_str()) {
                        // path_type is e.g. `types::ScalarAttributeType` (no crate prefix)
                        return format!("{}::{}::{}", crate_name, path_type, field);
                    }
                }
                // Lowercase variant on a stub-known type (e.g. BillingMode.pay_per_request
                // → aws_sdk_dynamodb::types::BillingMode::PayPerRequest).
                if !field_is_variant {
                    if let Some((crate_name, path_type)) = ctx.stub_type_crate.get(name.as_str()) {
                        // Convert snake_case variant to PascalCase
                        let variant: String = field
                            .split('_')
                            .map(|seg| {
                                let mut chars = seg.chars();
                                match chars.next() {
                                    Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                                    None => String::new(),
                                }
                            })
                            .collect();
                        return format!("{}::{}::{}", crate_name, path_type, variant);
                    }
                }
            }
            // Envelope routing: a field of a routing-returned local is a JSON
            // index (`result["code"]`). Envelope results are serde_json::Value.
            // Issue 2: Also applies to bus invoke results outside envelope routing.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return format!("{}[\"{}\"]", name, field);
                }
            }
            // Nested field access on a JSON value at any depth: result["a"]["b"]["c"]
            // When base resolves to a chain rooted in a JSON local, chain indexing.
            if is_json_rooted_expr(base, ctx) {
                let base_str = expr_to_rust(base, ctx);
                return format!("{}[\"{}\"]", base_str, field);
            }
            let base_str = expr_to_rust(base, ctx);
            // Auto-unwrap Option<T> locals on field access: if a local has type
            // `Option<X>`, field access implies the value is expected to be present.
            // Emit `.clone().ok_or(DomainError::NotFound)?.field` so the Option is
            // unwrapped at point of use.  This handles the common pattern where a
            // port method returns `Opt<T>` and the VEIL code accesses fields directly.
            // When the enclosing function returns Option<T>, use `?` directly
            // (returns None early) instead of converting to Result.
            if let Expr::Ident(name) = base.as_ref() {
                if let Some(ty) = ctx.local_type(name) {
                    if ty.starts_with("Option<") {
                        let enclosing_returns_option = ctx.expected_return_rust.as_ref()
                            .map(|r| r.starts_with("Option<"))
                            .unwrap_or(false);
                        if enclosing_returns_option {
                            return format!(
                                "{}.clone()?.{}",
                                base_str,
                                to_snake(field)
                            );
                        }
                        return format!(
                            "{}.clone().ok_or(DomainError::NotFound)?.{}",
                            base_str,
                            to_snake(field)
                        );
                    }
                }
            }
            // TODO: enum field access — when `base` is an enum instance (e.g.
            // `version.hash` where version is MetaFunctionVersion::Pinned { hash }),
            // Rust requires `if let Enum::Variant { field, .. } = base { field }`.
            // Without type info at codegen time we emit direct field access which
            // only works for structs. May need if-let destructuring when type info
            // is available.
            // VEIL field reads are reusable (not Rust moves). Clone non-Copy fields.
            let rust = format!("{}.{}", base_str, to_snake(field));
            if rust.ends_with(".clone()") || field_access_is_copy(base, field, ctx) {
                rust
            } else {
                format!("{rust}.clone()")
            }
        }
        Expr::Call(call) => translate_call(call, ctx),
        Expr::BinaryOp(op) => {
            let l = expr_to_rust(&op.left, ctx);
            let r = expr_to_rust(&op.right, ctx);
            // Special case: x != None → x.is_some(), x == None → x.is_none()
            if r == "None" {
                return match op.op {
                    veil_ir::ast::BinOp::NotEq => format!("{}.is_some()", l),
                    veil_ir::ast::BinOp::Eq => format!("{}.is_none()", l),
                    _ => format!("{} {} {}", l, binop_to_rust(&op.op), r),
                };
            }
            if l == "None" {
                return match op.op {
                    veil_ir::ast::BinOp::NotEq => format!("{}.is_some()", r),
                    veil_ir::ast::BinOp::Eq => format!("{}.is_none()", r),
                    _ => format!("{} {} {}", l, binop_to_rust(&op.op), r),
                };
            }
            // List append: `out + [x]` / `out + vec` → extend into owned Vec
            if matches!(op.op, veil_ir::ast::BinOp::Add)
                && (r.starts_with("vec![") || l.starts_with("vec!["))
            {
                return format!(
                    "{{ let mut __v = {l}; __v.extend({r}); __v }}"
                );
            }
            // String concat: Rust `String` has no `+ &String` / `+ &str` mix that
            // typechecks for every operand shape. `format!` is the portable
            // lowering for VEIL `Str + Str` (and `"lit" + field`).
            if matches!(op.op, veil_ir::ast::BinOp::Add)
                && (expr_is_stringish(&op.left, &l, ctx) || expr_is_stringish(&op.right, &r, ctx))
                && !(expr_is_numeric(&op.left, ctx) && expr_is_numeric(&op.right, ctx))
            {
                let l = clone_if_named_value(&op.left, l);
                let r = clone_if_named_value(&op.right, r);
                return format!("format!(\"{{}}{{}}\", {l}, {r})");
            }
            format!("{} {} {}", l, binop_to_rust(&op.op), r)
        }
        Expr::UnaryOp(op) => {
            let inner = expr_to_rust(&op.expr, ctx);
            format!("{}{}", unaryop_to_rust(&op.op), inner)
        }
        Expr::IfExpr(ie) => {
            let mut cond_ctx = ctx.clone_for_inference();
            cond_ctx.option_value_wrap = false;
            let cond = expr_to_rust(&ie.condition, &cond_ctx);
            // Auto-coerce serde_json::Value → bool for if conditions
            let cond = if let Expr::Ident(name) = ie.condition.as_ref() {
                if ctx.local_type(name) == Some("serde_json::Value") {
                    format!("{}.as_bool().unwrap_or(false)", name)
                } else { cond }
            } else { cond };
            // Single-expression if/else: emit as value expression (no semicolons)
            if ie.then_body.len() == 1 && ie.else_body.as_ref().map_or(false, |b| b.len() == 1) {
                let then_expr = expr_to_rust(&ie.then_body[0], ctx);
                let else_expr = expr_to_rust(&ie.else_body.as_ref().unwrap()[0], ctx);
                return format!("if {} {{ {} }} else {{ {} }}", cond, then_expr, else_expr);
            }
            if ctx.option_value_wrap {
                let then_body = emit_value_block(&ie.then_body, ctx, "    ");
                if let Some(else_body) = &ie.else_body {
                    let else_stmts = emit_value_block(else_body, ctx, "    ");
                    return format!(
                        "if {} {{\n{}\n}} else {{\n{}\n}}",
                        cond, then_body, else_stmts
                    );
                }
                return format!("if {} {{\n{}\n}} else {{\n    None\n}}", cond, then_body);
            }
            let then_body = emit_tracked_block(&ie.then_body, ctx, "    ");
            if let Some(else_body) = &ie.else_body {
                let else_stmts = emit_tracked_block(else_body, ctx, "    ");
                format!("if {} {{\n{}\n}} else {{\n{}\n}}", cond, then_body, else_stmts)
            } else {
                format!("if {} {{\n{}\n}}", cond, then_body)
            }
        }
        Expr::Assign(name, rhs, ty_ann) => {
            // List append sugar: `out = out + [x]` → `out.push(x)` when the
            // left is the same local and the right is a single-element list.
            if let Expr::BinaryOp(bin) = rhs.as_ref() {
                if matches!(bin.op, veil_ir::ast::BinOp::Add) {
                    if let (Expr::Ident(left), Expr::ArrayLit(items)) =
                        (bin.left.as_ref(), bin.right.as_ref())
                    {
                        if left == name && items.len() == 1 {
                            let item = expr_to_rust(&items[0], ctx);
                            // Auto-unwrap Option<T> items pushed into a list: if
                            // the item is a local with Option<T> type, unwrap it
                            // since the list expects T elements.
                            if let Expr::Ident(item_name) = &items[0] {
                                if let Some(ty) = ctx.local_type(item_name) {
                                    if ty.starts_with("Option<") {
                                        return format!(
                                            "{}.push({}.clone().ok_or(DomainError::NotFound)?)",
                                            name, item
                                        );
                                    }
                                }
                            }
                            return format!("{}.push({})", name, item);
                        }
                    }
                }
            }
            // List concat sugar: `x = x.concat([items])` → `x.extend(vec![items])`
            // when target == LHS name and arg is an array literal.
            if let Expr::Call(call) = rhs.as_ref() {
                let bare_m = call.method.trim_end_matches('!');
                if bare_m == "concat" && call.target == *name && !call.args.is_empty() {
                    if let Some(Expr::ArrayLit(items)) = call.args.first() {
                        let item_strs: Vec<String> = items.iter().map(|i| expr_to_rust(i, ctx)).collect();
                        if items.len() == 1 {
                            return format!("{}.push({})", name, item_strs[0]);
                        } else {
                            return format!("{}.extend(vec![{}])", name, item_strs.join(", "));
                        }
                    }
                }
            }
            let rhs_str = expr_to_rust(rhs, ctx);
            // Field assignment: `wt.name = x` stored as Assign("wt.name", …)
            // Emit path with snake_case fields; never introduce a `let` binding.
            if name.contains('.') {
                let parts: Vec<&str> = name.splitn(2, '.').collect();
                let base_name = parts[0];
                let field_path = parts[1];
                // Auto-unwrap Option<T> locals on field assignment: if the base
                // local is Option<T>, we need to unwrap it first. Use
                // `as_mut().ok_or(DomainError::NotFound)?.field = val` pattern.
                if let Some(ty) = ctx.local_type(base_name) {
                    if ty.starts_with("Option<") {
                        let field_snake = field_path
                            .split('.')
                            .map(|s| to_snake(s))
                            .collect::<Vec<_>>()
                            .join(".");
                        return format!(
                            "{}.as_mut().ok_or(DomainError::NotFound)?.{} = {}",
                            base_name, field_snake, rhs_str
                        );
                    }
                }
                let path = name
                    .split('.')
                    .enumerate()
                    .map(|(i, seg)| {
                        if i == 0 {
                            seg.to_string()
                        } else {
                            to_snake(seg)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(".");
                return format!("{} = {}", path, rhs_str);
            }
            if ctx.state_locals.contains(name.as_str()) {
                // Write the result into the threaded step state as JSON.
                format!("state[\"{}\"] = serde_json::json!({})", name, rhs_str)
            } else if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                format!("self.{} = {}", to_snake(name), rhs_str)
            } else if ctx.is_local(name) {
                // Already-declared local (e.g. a `mut` var) → reassignment, no `let`.
                format!("{} = {}", name, rhs_str)
            } else {
                let mut_kw = if ctx.mut_locals.contains(name.as_str()) {
                    "mut "
                } else {
                    ""
                };
                if let Some(ty) = ty_ann {
                    format!(
                        "let {}{}: {} = {}",
                        mut_kw,
                        name,
                        crate::rust::type_to_rust(ty),
                        rhs_str
                    )
                } else {
                    format!("let {}{} = {}", mut_kw, name, rhs_str)
                }
            }
        }
        Expr::MutAssign(name, rhs, ty_ann) => {
            // List concat sugar: `x = x.concat([items])` → `x.extend(vec![items])`
            if let Expr::Call(call) = rhs.as_ref() {
                let bare_m = call.method.trim_end_matches('!');
                if bare_m == "concat" && call.target == *name && !call.args.is_empty() {
                    if let Some(Expr::ArrayLit(items)) = call.args.first() {
                        let item_strs: Vec<String> = items.iter().map(|i| expr_to_rust(i, ctx)).collect();
                        if items.len() == 1 {
                            return format!("{}.push({})", name, item_strs[0]);
                        } else {
                            return format!("{}.extend(vec![{}])", name, item_strs.join(", "));
                        }
                    }
                }
            }
            let rhs_str = expr_to_rust(rhs, ctx);
            // Reassignment of an already-bound local (e.g. `mut req` inside while).
            if ctx.is_local(name) {
                return format!("{} = {}", name, rhs_str);
            }
            match ty_ann {
                Some(ty) => format!("let mut {}: {} = {}", name, crate::rust::type_to_rust(ty), rhs_str),
                None => format!("let mut {} = {}", name, rhs_str),
            }
        }
        Expr::StringLit(s) => format!("\"{}\".to_string()", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => {
            // `ret Ok` / `ret Err e` construct the Result directly; anything
            // else is the success value and gets wrapped in `Ok(..)`.
            match inner.as_ref() {
                Expr::Ident(n) if n == "Ok" => "return Ok(())".to_string(),
                Expr::Ident(n) if n == "Err" => {
                    "return Err(DomainError::External(\"error\".to_string()))".to_string()
                }
                // `ret Err e` parses as a call `Err(e)` or ident chain; handle a
                // call whose target is Err.
                Expr::Call(c) if c.target == "Err" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
                    if a.is_empty() {
                        "return Err(DomainError::Validation(\"error\".to_string()))".to_string()
                    } else if a.starts_with("DomainError::") {
                        // Already a DomainError variant
                        format!("return Err({})", a)
                    } else {
                        // Check if the argument is a simple identifier (likely a caught error variable)
                        let is_simple_ident = c.args.len() == 1 && matches!(&c.args[0], Expr::Ident(_));
                        if is_simple_ident {
                            // Bare variable from a match arm — likely already DomainError
                            format!("return Err({})", a)
                        } else if matches!(c.args.first(), Some(Expr::StringLit(_))) {
                            // ret Err "msg" → External (adapter fail-closed, not validation)
                            format!("return Err(DomainError::External({}))", a)
                        } else {
                            // format! / computed messages (upstream HTTP, DB) → External → 502
                            // User-facing validation uses `guard`, not `ret Err`.
                            format!("return Err(DomainError::External({}))", a)
                        }
                    }
                }
                Expr::Call(c) if c.target == "Ok" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
                    format!("return Ok({})", if a.is_empty() { "()".to_string() } else { a })
                }
                _ => {
                    let val = expr_to_rust(inner, ctx);
                    // Check if the function returns Result<...> — if so, wrap in Ok().
                    let returns_result = ctx
                        .expected_return_rust
                        .as_deref()
                        .map(|t| t.starts_with("Result<"))
                        .unwrap_or(true); // default to Result wrapping
                    let returns_option = ctx
                        .expected_return_rust
                        .as_deref()
                        .map(|t| t.contains("Option<"))
                        .unwrap_or(false);
                    if !returns_result {
                        // Direct return (not Result-wrapped)
                        if val == "None" {
                            if returns_option {
                                "return None".to_string()
                            } else {
                                // Non-Option API with null → treat as missing resource
                                "return /* null */ unreachable!(\"null return on non-Option\")"
                                    .to_string()
                            }
                        } else if returns_option && !val.starts_with("Some(") {
                            format!("return Some({})", val)
                        } else {
                            format!("return {}", val)
                        }
                    } else if val == "None" || val == "()" {
                        // `ret null` / `ret ()`: Option APIs → Ok(None); otherwise NotFound / unit Ok.
                        if returns_option {
                            "return Ok(None)".to_string()
                        } else if val == "()" {
                            "return Ok(())".to_string()
                        } else {
                            "return Err(DomainError::NotFound)".to_string()
                        }
                    } else if returns_option && !val.starts_with("Some(") {
                        // If the value is already Option<T> (from a local typed as such),
                        // don't double-wrap in Some(). Just return Ok(val).
                        if let Expr::Ident(name) = inner.as_ref() {
                            if ctx.local_type(name).map(|t| t.starts_with("Option<")).unwrap_or(false) {
                                return format!("return Ok({})", val);
                            }
                        }
                        format!("return Ok(Some({}))", val)
                    } else {
                        format!("return Ok({})", val)
                    }
                }
            }
        }
        Expr::Await(inner) => {
            let inner_str = expr_to_rust(inner, ctx);
            format!("{}.await", inner_str)
        }
        Expr::Break => "break".to_string(),
        Expr::Continue => "continue".to_string(),
        Expr::Index(base, idx) => {
            let b = expr_to_rust(base, ctx);
            // HashMap / Dynamo item: `.get("key").cloned().ok_or(NotFound)?`
            // so subsequent `.as_s()` is on AttributeValue, not Option.
            match idx.as_ref() {
                Expr::StringLit(s) => format!(
                    "{b}.get(\"{s}\").cloned().ok_or(DomainError::NotFound)?"
                ),
                // Dynamic key (e.g. params[p.name] on serde_json::Value)
                other => {
                    let i = expr_to_rust(other, ctx);
                    let base_ty = match base.as_ref() {
                        Expr::Ident(n) => ctx.local_type(n).unwrap_or(""),
                        _ => "",
                    };
                    // Integer / usize indices → slice/vec indexing, never `.as_str()`.
                    let idx_is_int = matches!(other, Expr::IntLit(_))
                        || matches!(
                            other,
                            Expr::Ident(n) if matches!(
                                ctx.local_type(n),
                                Some("i64")
                                    | Some("i32")
                                    | Some("u64")
                                    | Some("u32")
                                    | Some("usize")
                                    | Some("isize")
                            )
                        );
                    if idx_is_int {
                        format!("{b}[({i}) as usize]")
                    } else if base_ty.contains("Value") || base_ty == "Json" || base_ty.is_empty()
                    {
                        // String-keyed JSON map access.
                        format!(
                            "{b}.get({i}.as_str()).cloned().unwrap_or(serde_json::Value::Null)"
                        )
                    } else {
                        format!("{b}[{i}]")
                    }
                }
            }
        }
        Expr::ArrayLit(items) => { let s = items.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", "); format!("vec![{}]", s) }
        Expr::Range { start, end, inclusive } => { let s = start.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default(); let e = end.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default(); let op = if *inclusive { "..=" } else { ".." }; format!("{}{}{}", s, op, e) }
        Expr::Loop(body) => { let b = body.iter().map(|e| format!("    {};", expr_to_rust(e, ctx))).collect::<Vec<_>>().join("\n"); format!("loop {{\n{}\n}}", b) }
        Expr::DoBlock(body) => {
            if body.is_empty() {
                "{}".to_string()
            } else {
                // Child scope for type tracking — locals don't leak out of the block
                let mut block_ctx = ctx.clone_for_inference();
                let mut lines = Vec::new();
                for (i, e) in body.iter().enumerate() {
                    let rust = expr_to_rust(e, &block_ctx);
                    // Track local types so subsequent lines resolve receiver types
                    if let Expr::Assign(name, rhs, ty_ann) | Expr::MutAssign(name, rhs, ty_ann) = e {
                        if !name.contains('.') {
                            block_ctx.locals.insert(name.clone());
                            if let Some(ty) = ty_ann {
                                block_ctx.local_types.insert(name.clone(), crate::rust::type_to_rust(ty));
                            } else if let Some(t) = infer_expr_type(rhs, &block_ctx) {
                                block_ctx.local_types.insert(name.clone(), t);
                            }
                        }
                    }
                    if i == body.len() - 1 {
                        // Last expression: no semicolon (block return value)
                        lines.push(format!("    {}", rust));
                    } else {
                        lines.push(format!("    {};", rust));
                    }
                }
                format!("{{\n{}\n}}", lines.join("\n"))
            }
        }
        Expr::Cast(expr, ty) => format!("{} as {}", expr_to_rust(expr, ctx), ty),
        Expr::Try(expr) => format!("{}?", expr_to_rust(expr, ctx)),
        Expr::Require(inner) => {
            let s = expr_to_rust(inner, ctx);
            // ACS-010: require force-presents one Opt layer *and* one Res layer.
            // Bang already emits try (`?` / `.await?`) for Res. If the success
            // type is still Option, we must unwrap that too — do not treat a
            // trailing `?` as "already fully present".
            let still_option = infer_expr_type(inner, ctx)
                .as_deref()
                .is_some_and(|t| peel_option_rust(t).is_some());
            if still_option {
                format!("{s}.ok_or(DomainError::NotFound)?")
            } else if s.trim_end().ends_with('?') {
                s
            } else {
                format!("{s}.ok_or(DomainError::NotFound)?")
            }
        },
        Expr::StructUpdate { name, fields, base } => { let fs = fields.iter().map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx))).collect::<Vec<_>>().join(", "); format!("{} {{ {}, ..{} }}", name, fs, expr_to_rust(base, ctx)) }
        Expr::IfLet { pattern, expr, then_body, else_body } => {
            let e = expr_to_rust(expr, ctx);
            let then_str = then_body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            let else_str = else_body.as_ref().map(|eb| { let s = eb.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n"); format!(" else {{\n{}\n}}", s) }).unwrap_or_default();
            format!("if let {} = {} {{\n{}\n}}{}", pattern, e, then_str, else_str)
        }
        Expr::WhileLet { pattern, expr, body } => {
            let e = expr_to_rust(expr, ctx);
            let body_str = body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            format!("while let {} = {} {{\n{}\n}}", pattern, e, body_str)
        }
        Expr::LetPattern(pattern, expr, ty_ann) => {
            let pat_str = pattern_to_rust(pattern);
            let e = expr_to_rust(expr, ctx);
            match ty_ann {
                Some(ty) => format!("let {}: {} = {}", pat_str, crate::rust::type_to_rust(ty), e),
                None => format!("let {} = {}", pat_str, e),
            }
        }
        Expr::Action(a) => translate_action(a, ctx),
        Expr::StructLit(name, fields) if name.is_empty() => {
            // Anonymous record/map literal (`{}` or `{ key: value, ... }`) → a
            // JSON object value.
            if fields.is_empty() {
                "serde_json::json!({})".to_string()
            } else {
                let pairs = fields.iter().map(|(k, v)| {
                    format!("\"{}\": {}", k, to_json_arg(v, ctx))
                }).collect::<Vec<_>>().join(", ");
                format!("serde_json::json!({{ {} }})", pairs)
            }
        }
        Expr::StructLit(name, fields) => {
            let fs = fields.iter().map(|(k, v)| {
                let v_str = expr_to_rust(v, ctx);
                // Clone ident and field access values to prevent move issues.
                // Skip copy/null/bools so we don't emit `None.clone()`.
                let cloned = match v {
                    Expr::Ident(n)
                        if n == "null"
                            || n == "true"
                            || n == "false"
                            || is_copy_local(n, ctx) =>
                    {
                        v_str.clone()
                    }
                    Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("{}.clone()", v_str),
                    _ => v_str.clone(),
                };
                // Type-aware coercion: when a field value is serde_json::Value
                // but the target struct field expects a typed value, auto-convert.
                // Also handle the reverse: typed values going into Json/Option<Json> fields.
                let coerced = if let Some(field_ty) = ctx.field_type(name, k) {
                    let val_ty = match v {
                        Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
                        _ => infer_expr_type(v, ctx),
                    };
                    if val_ty.as_deref() == Some("serde_json::Value") {
                        match field_ty {
                            "String" => format!("{}.as_str().unwrap_or(\"\").to_string()", cloned.trim_end_matches(".clone()")),
                            "bool" => format!("{}.as_bool().unwrap_or(false)", cloned.trim_end_matches(".clone()")),
                            "i64" => format!("{}.as_i64().unwrap_or(0)", cloned.trim_end_matches(".clone()")),
                            "f64" => format!("{}.as_f64().unwrap_or(0.0)", cloned.trim_end_matches(".clone()")),
                            t if t.starts_with("Option<") => format!("Some({})", cloned),
                            _ => cloned,
                        }
                    } else if field_ty == "serde_json::Value" || field_ty == "Option<serde_json::Value>" {
                        // Non-JSON value going into a Json field → wrap with json!()
                        if field_ty.starts_with("Option") {
                            // null → None (not Some(json!(None)))
                            if cloned == "None" {
                                "None".to_string()
                            } else {
                                format!("Some(serde_json::json!({}))", cloned)
                            }
                        } else {
                            format!("serde_json::json!({})", cloned)
                        }
                    } else {
                        cloned
                    }
                } else {
                    cloned
                };
                if k == &v_str { format!("{}: {}.clone()", k, k) } else { format!("{}: {}", to_snake(k), coerced) }
            }).collect::<Vec<_>>().join(", ");
            format!("{} {{ {} }}", name, fs)
        }
        Expr::Match(scrutinee, arms) => {
            // The match consumes the scrutinee's Result directly, so a fallible
            // call scrutinee must NOT auto-propagate with `?`.
            // Never Some-wrap the scrutinee — only arm values.
            let mut scrut_ctx = ctx.clone_for_inference();
            scrut_ctx.option_value_wrap = false;
            let raw = expr_to_rust(scrutinee, &scrut_ctx);
            // String-literal arms match `&str`. Keep the try-unwrap so we do
            // not call `.as_str()` on a `Result`. Result/enum arms strip `?`
            // so the match can consume Ok/Err or the domain value.
            let has_string_patterns = arms.iter().any(|a| a.pattern.starts_with('"'));
            let scrutinee_str = if has_string_patterns {
                raw.clone()
            } else {
                strip_try_suffix(raw)
            };
            // If the scrutinee is a serde_json::Value local but arms use typed
            // enum/struct patterns, deserialize first.
            let scrutinee_str = if let Expr::Ident(name) = scrutinee.as_ref() {
                if ctx.local_type(name) == Some("serde_json::Value") {
                    // Detect enum type from first arm's pattern (e.g. "ReconcileResult.InSync")
                    let first_pat = arms.first().map(|a| &a.pattern).cloned().unwrap_or_default();
                    let has_enum_pat = first_pat.contains("::")
                        || first_pat.contains('.')
                        || first_pat.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
                    if has_enum_pat && !first_pat.starts_with('"') && first_pat != "_" {
                        // Extract enum type: "ReconcileResult.InSync" → "ReconcileResult"
                        // or "ReconcileResult::InSync" → "ReconcileResult"
                        let enum_type = first_pat.split(|c| c == '.' || c == ':')
                            .next().unwrap_or(&first_pat)
                            .split('{').next().unwrap_or(&first_pat).trim();
                        format!("serde_json::from_value::<{}>({}.clone()).unwrap()", enum_type, name)
                    } else {
                        scrutinee_str
                    }
                } else {
                    scrutinee_str
                }
            } else {
                scrutinee_str
            };
            // String-literal arms need `&str`. Scrutinee is already unwrapped
            // (see above) so this is String / &str, never Result.
            let scrutinee_final = if has_string_patterns {
                let t = scrutinee_str.trim();
                if t.ends_with(".as_str()") || t.ends_with(".as_str().trim()") {
                    scrutinee_str
                } else {
                    format!("{scrutinee_str}.as_str()")
                }
            } else {
                scrutinee_str
            };
            // When the scrutinee is a local variable matched against enum
            // patterns with field destructuring, clone it so the variable is
            // not moved and can be reused in subsequent match expressions or
            // later statements. Pattern bindings stay owned (no ref issues).
            let has_enum_patterns = arms.iter().any(|a| a.pattern.contains('.') || a.pattern.contains("::"));
            let scrutinee_is_local_ident = if let Expr::Ident(name) = scrutinee.as_ref() {
                ctx.is_local(name) && !has_string_patterns
            } else {
                false
            };
            let scrutinee_final = if scrutinee_is_local_ident && has_enum_patterns {
                format!("{}.clone()", scrutinee_final)
            } else {
                scrutinee_final
            };
            let mut out = format!("match {} {{\n", scrutinee_final);
            for arm in arms {
                // Use structured pattern if available, fall back to string normalization
                let pattern = if let Some(rich) = &arm.rich_pattern {
                    pattern_to_rust_qualified(rich, Some(&ctx.enum_variants))
                } else {
                    normalize_match_pattern(&arm.pattern, ctx)
                };
                let guard_str = match &arm.guard {
                    Some(g) => format!(" if {}", expr_to_rust(g, &scrut_ctx)),
                    None => String::new(),
                };
                // Match arm bodies get their own local set (bindings + assigns).
                let mut arm_ctx = ctx.clone_for_inference();
                // Bind pattern idents as locals (Some(item) → item)
                for name in pattern_binding_names(&arm.pattern) {
                    arm_ctx.locals.insert(name);
                }
                arm_ctx.mut_locals.extend(analyze_mut_locals(&arm.body));
                let body_str = if arm.body.len() == 1 {
                    expr_to_rust(&arm.body[0], &arm_ctx)
                } else {
                    format!(
                        "{{\n{}\n    }}",
                        emit_value_block(&arm.body, &arm_ctx, "        ")
                    )
                };
                out.push_str(&format!("        {}{} => {},\n", pattern, guard_str, body_str));
            }
            // Add wildcard arm for enum matches to ensure exhaustiveness
            let has_enum_patterns = arms.iter().any(|a| a.pattern.contains('.') || a.pattern.contains("::"));
            let has_wildcard = arms.iter().any(|a| a.pattern == "_" || a.pattern == "else" || a.pattern.starts_with('_'));
            if has_enum_patterns && !has_wildcard {
                out.push_str("        _ => unreachable!()\n");
            }
            out.push_str("    }");
            out
        }
        Expr::ForLoop { binding, index, iterable, body } => {
            let mut iter_str = expr_to_rust(iterable, ctx);
            // Avoid moving struct fields reused across loops (while + second for).
            // Skip bare idents: they are often already `&[T]` (e.g. Dynamo items()).
            if matches!(iterable.as_ref(), Expr::FieldAccess(_, _))
                && !iter_str.ends_with(".clone()")
                && !iter_str.ends_with(".iter()")
            {
                iter_str = format!("{iter_str}.clone()");
            }
            let bind = if let Some(idx) = index {
                format!("({}, {})", idx, binding)
            } else {
                binding.clone()
            };
            // The loop variable is a local within the body. Infer its element
            // type from the iterable so method calls on it resolve (e.g. a
            // `List<SagaStep>` yields `SagaStep` elements).
            let mut body_ctx = ctx.clone_for_inference();
            body_ctx.locals.insert(binding.clone());
            if let Some(elem) = element_type_of(iterable, ctx) {
                body_ctx.local_types.insert(binding.clone(), elem);
            }
            if let Some(idx) = index {
                body_ctx.locals.insert(idx.clone());
            }
            body_ctx.mut_locals.extend(analyze_mut_locals(body));
            let mut body_lines = Vec::new();
            for e in body {
                let line = expr_to_rust(e, &body_ctx);
                if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = e {
                    if !name.contains('.') {
                        body_ctx.locals.insert(name.clone());
                        // Infer and track the type so subsequent statements can use it
                        // (e.g. for Option<T> auto-unwrap in push calls).
                        if let Some(t) = infer_expr_type(rhs, &body_ctx) {
                            body_ctx.local_types.insert(name.clone(), t);
                        }
                    }
                }
                body_lines.push(format!("        {};", line));
            }
            let body_str = body_lines.join("\n");
            let enumerate = if index.is_some() { ".enumerate()" } else { "" };
            // If the iterable type is Option<_>, unwrap to empty default; else as-is.
            let iter_expr = if let Expr::Ident(name) = iterable.as_ref() {
                if ctx
                    .local_type(name)
                    .map(|t| t.starts_with("Option<"))
                    .unwrap_or(false)
                {
                    format!("{iter_str}.unwrap_or_default()")
                } else {
                    iter_str
                }
            } else {
                iter_str
            };
            format!("for {} in {}{} {{\n{}\n    }}", bind, iter_expr, enumerate, body_str)
        }
        Expr::WhileLoop { condition, body } => {
            let cond_str = expr_to_rust(condition, ctx);
            // Track locals across the loop body so `mut req = …` then `req = …`
            // reassigns (adapters / retries) instead of shadowing or free fns.
            let mut body_ctx = ctx.clone_for_inference();
            body_ctx.mut_locals.extend(analyze_mut_locals(body));
            let mut lines = Vec::new();
            for e in body {
                let line = expr_to_rust(e, &body_ctx);
                if let Expr::Assign(name, _, _) | Expr::MutAssign(name, _, _) = e {
                    if !name.contains('.') {
                        body_ctx.locals.insert(name.clone());
                    }
                }
                lines.push(format!("        {};", line));
            }
            format!("while {} {{\n{}\n    }}", cond_str, lines.join("\n"))
        }
        Expr::Tuple(items) => {
            let parts = items.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        Expr::StringInterp(parts) => {
            use veil_ir::ast::StringPart;
            let mut fmt = String::new();
            let mut args = Vec::new();
            for p in parts {
                match p {
                    // Escape `{`/`}` so literal braces survive `format!` (e.g. path `{id}`).
                    StringPart::Literal(l) => {
                        for ch in l.chars() {
                            match ch {
                                '{' => fmt.push_str("{{"),
                                '}' => fmt.push_str("}}"),
                                _ => fmt.push(ch),
                            }
                        }
                    }
                    StringPart::Expr(e) => {
                        fmt.push_str("{}");
                        args.push(expr_to_rust(e, ctx));
                    }
                }
            }
            if args.is_empty() {
                // Still a format-free string; unescape was only for format! — rebuild raw.
                let raw: String = parts
                    .iter()
                    .filter_map(|p| match p {
                        StringPart::Literal(l) => Some(l.as_str()),
                        _ => None,
                    })
                    .collect();
                format!("\"{}\".to_string()", raw.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                format!("format!(\"{}\", {})", fmt, args.join(", "))
            }
        }
        Expr::Closure { params, body } => {
            let p = params.join(", ");
            // Closure bodies don't return Result, so `?` operator isn't valid.
            // Replace `?` with `.unwrap()` for fallible expressions inside closures.
            let fixup_closure_body = |s: String| -> String {
                s.replace(".map_err(|e| DomainError::External(format!(\"{:?}\", e)))?", ".unwrap()")
                 .replace(".map_err(|e| DomainError::External(format!(\"{e:?}\")))?", ".unwrap()")
                 .replace(".map_err(|e| DomainError::External(e.to_string()))?", ".unwrap()")
            };
            // Replace trailing `?` from serde/other fallible calls with `.unwrap()`
            let fixup_question = |mut s: String| -> String {
                // Pattern: `expr)?` → `expr).unwrap()`
                while let Some(pos) = s.find(")?") {
                    // Only replace if not inside a larger pattern (e.g. `.map_err(...)? `)
                    let after = if pos + 2 < s.len() { &s[pos+2..pos+3] } else { "" };
                    if after.is_empty() || after == ")" || after == "." || after == "," || after == ";" || after == " " {
                        s = format!("{}).unwrap(){}", &s[..pos], &s[pos+2..]);
                    } else {
                        break;
                    }
                }
                s
            };
            // Clone ctx and add closure params as locals so that calls on
            // them (e.g. `item.field()`) resolve as method calls, not external-
            // effect hooks.
            let mut closure_ctx = ctx.clone_for_inference();
            for param in params {
                closure_ctx.locals.insert(param.clone());
            }
            if body.len() == 1 {
                let body_str = expr_to_rust(&body[0], &closure_ctx);
                let body_str = fixup_closure_body(body_str);
                let body_str = fixup_question(body_str);
                format!("|{}| {}", p, body_str)
            } else {
                let stmts = body.iter()
                    .map(|e| {
                        let s = expr_to_rust(e, &closure_ctx);
                        let s = fixup_closure_body(s);
                        let s = fixup_question(s);
                        format!("    {};", s)
                    })
                    .collect::<Vec<_>>().join("\n");
                format!("|{}| {{\n{}\n}}", p, stmts)
            }
        }
        // Expanded by adapt merge before codegen — should never remain.
        Expr::Stock => {
            "/* error: stock not expanded */ ()".to_string()
        }
    }
}

/// Render an expression for embedding inside a `json!` payload. Values are
/// Issue 5: Translate inline VEIL ternary expressions with nested f-strings.
/// Input: `if x.is_some() then f" in {x.unwrap()}" else ""`
/// Output: `if x.is_some() { format!(" in {}", x.unwrap()) } else { "".to_string() }`
fn translate_inline_ternary_fstring(raw: &str) -> String {
    // Parse: `if <cond> then <then_expr> else <else_expr>`
    let Some(then_idx) = raw.find(" then ") else {
        return raw.to_string();
    };
    let cond = &raw[3..then_idx]; // skip "if "
    let after_then = &raw[then_idx + 6..]; // skip " then "

    // Find the `else` boundary — must handle nested quotes
    let (then_expr, else_expr) = if let Some(else_idx) = find_top_level_else(after_then) {
        (&after_then[..else_idx], after_then[else_idx + 5..].trim()) // skip " else "
    } else {
        (after_then, "\"\"")
    };

    let then_rust = translate_fstring_value(then_expr.trim());
    let else_rust = translate_fstring_value(else_expr.trim());

    format!("if {} {{ {} }} else {{ {} }}", cond, then_rust, else_rust)
}

/// Find top-level " else " that's not inside quotes.
fn find_top_level_else(s: &str) -> Option<usize> {
    let mut in_quote = false;
    let mut quote_char = '"';
    let bytes = s.as_bytes();
    let else_pat = b" else ";
    for i in 0..s.len().saturating_sub(5) {
        let ch = bytes[i] as char;
        if !in_quote && (ch == '"' || ch == '\'') {
            in_quote = true;
            quote_char = ch;
        } else if in_quote && ch == quote_char && (i == 0 || bytes[i - 1] != b'\\') {
            in_quote = false;
        } else if !in_quote && i + 6 <= s.len() && &bytes[i..i + 6] == else_pat {
            return Some(i);
        }
    }
    None
}

/// Translate a value that may be an f-string or a plain string literal.
fn translate_fstring_value(val: &str) -> String {
    // f"..." or f'...' → format!(...)
    if (val.starts_with("f\"") && val.ends_with('"')) ||
       (val.starts_with("f'") && val.ends_with('\'')) {
        let inner = &val[2..val.len() - 1];
        // Convert {expr} interpolations to format! args
        let mut fmt = String::new();
        let mut args = Vec::new();
        let mut chars = inner.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '{' {
                let mut depth = 1;
                let mut expr_text = String::new();
                while let Some(c) = chars.next() {
                    if c == '{' { depth += 1; }
                    if c == '}' { depth -= 1; if depth == 0 { break; } }
                    expr_text.push(c);
                }
                fmt.push_str("{}");
                args.push(expr_text);
            } else {
                fmt.push(ch);
            }
        }
        if args.is_empty() {
            format!("\"{}\".to_string()", fmt)
        } else {
            format!("format!(\"{}\", {})", fmt, args.join(", "))
        }
    } else if val.starts_with('"') && val.ends_with('"') {
        // Plain string literal
        format!("{}.to_string()", val)
    } else if val.starts_with('\'') && val.ends_with('\'') {
        let inner = &val[1..val.len() - 1];
        format!("\"{}\".to_string()", inner)
    } else {
        val.to_string()
    }
}

/// Serialize field-access and method-call expressions to JSON-safe values for use as
/// cloned to avoid moving locals that are reused across bus calls; bare
/// non-local identifiers (e.g. enum variants like `FreeTier`) become JSON
/// strings; field access uses JSON indexing on the serialized base so it works
/// regardless of the (opaque) source type.
fn to_json_arg(expr: &Expr, ctx: &GenCtx) -> String {
    match expr {
        Expr::Ident(name) => {
            // VEIL null in JSON envelopes must be JSON null, not the string "null".
            if name == "null" {
                return "serde_json::Value::Null".to_string();
            }
            // A shared step-state value → read from the threaded state.
            if ctx.state_locals.contains(name.as_str()) {
                format!("state[\"{}\"].clone()", name)
            } else if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                // A struct-captured input (step impl) → self.<field>.
                format!("self.{}.clone()", to_snake(name))
            } else if ctx.is_local(name) {
                format!("{}.clone()", name)
            } else {
                // Non-local bare ident in a payload → symbolic string (enum variant, marker).
                format!("\"{}\"", name)
            }
        }
        Expr::FieldAccess(base, field) => {
            // A field of a state-local → index into the threaded state.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return format!("state[\"{}\"][\"{}\"].clone()", name, field);
                }
            }
            // If the base is already a serde_json::Value local, index it directly.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return format!("{}[\"{}\"].clone()", name, field);
                }
            }
            // Otherwise serialize the base then index (works for opaque stub types;
            // Index yields Null on mismatch rather than panicking).
            format!("serde_json::json!({})[\"{}\"].clone()", to_json_arg(base, ctx), field)
        }
        // Empty arrays in json! context need explicit typing
        Expr::ArrayLit(items) if items.is_empty() => {
            "serde_json::Value::Array(vec![])".to_string()
        }
        Expr::ArrayLit(items) => {
            let vals: Vec<String> = items.iter().map(|e| to_json_arg(e, ctx)).collect();
            format!("vec![{}]", vals.join(", "))
        }
        _ => expr_to_rust(expr, ctx),
    }
}

/// Determine the call suffix for a method invoked on a chained receiver.
///
/// - Fluent `.send()` / `.send_with()` are async + Result → `.await?`
/// - Stub methods marked async+fallible (BoxFuture / executor param) → `.await.map_err…?`
/// - Other stub methods marked `Res!` are sync Result → `map_err…?`
/// - Trait methods (ports) are async_trait + Result → `.await?`
///
/// **Receiver shape wins over bare method name.** The same identifier can name a
/// method on both a port trait and a stub/struct; suffix choice must follow the
/// *receiver's* Shape (Struct vs Trait) when known, not a global method-name scan.
///
/// Method names may carry VEIL bang/query suffixes (`fetch_all!`); strip before lookup.
fn receiver_call_suffix(recv: &Expr, method: &str, ctx: &GenCtx) -> String {
    let has_bang = method.ends_with('!');
    let method = method.trim_end_matches(['!', '?']);

    // Resolve the static type of the receiver when we can (UFCS / local / self field).
    // Index into List/slice of trait objects also yields a trait receiver
    // (e.g. `steps[i].action(...)` for `List<SagaStep>`).
    let recv_type_name: Option<String> = match recv {
        Expr::Ident(name) => {
            if ctx.is_struct_target(name) || ctx.is_trait_target(name) {
                Some(name.clone())
            } else if let Some(t) = ctx.local_type(name) {
                Some(t.to_string())
            } else if let Some(t) = ctx
                .self_field_types
                .get(name)
                .or_else(|| ctx.self_field_types.get(&to_snake(name)))
            {
                Some(
                    peel_dyn_trait_name(t)
                        .unwrap_or_else(|| t.clone()),
                )
            } else if ctx.stub_type_crate.contains_key(name) {
                Some(name.clone())
            } else {
                None
            }
        }
        Expr::Index(base, _) => {
            // List/slice element: peel Vec/slice and Box<dyn Trait>
            if let Expr::Ident(name) = base.as_ref() {
                ctx.local_type(name)
                    .and_then(|t| extract_box_dyn_trait(t).or_else(|| extract_vec_elem(t)))
            } else {
                None
            }
        }
        // AST still has `.get(i)` before list-index lowering; treat as element access.
        Expr::Call(inner)
            if (inner.method == "get" || inner.method == "get!") && inner.args.len() == 1 =>
        {
            let base_name = if !inner.target.is_empty() {
                Some(inner.target.as_str())
            } else if let Some(r) = &inner.receiver {
                match r.as_ref() {
                    Expr::Ident(n) => Some(n.as_str()),
                    _ => None,
                }
            } else {
                None
            };
            base_name.and_then(|n| {
                ctx.local_type(n)
                    .and_then(|t| extract_box_dyn_trait(t).or_else(|| extract_vec_elem(t)))
            })
        }
        _ => None,
    };

    // Known struct / stub type: use stub fallibility metadata only (not trait scan).
    if let Some(ref ty) = recv_type_name {
        // Peel Box<dyn Trait + …> / bare trait names stored in local_types
        let bare = peel_dyn_trait_name(ty).unwrap_or_else(|| ty.clone());
        if ctx.name_to_shape.get(bare.as_str()) == Some(&Shape::Struct)
            || ctx.stub_type_crate.contains_key(bare.as_str())
            || ctx.stub_type_crate.contains_key(ty.as_str())
        {
            if method == "send"
                || method == "send_with"
                || ctx.async_fallible_methods.contains(method)
            {
                // send!() → unwrap Result; bare send() keeps Result so .is_ok()/.is_err() work.
                if has_bang {
                    return ".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?".to_string();
                } else {
                    return ".await".to_string();
                }
            }
            if ctx.fallible_methods.contains(method) {
                let suffix = if should_own_str_result(ctx, Some(ty.as_str()), method) {
                    map_err_domain_own_str()
                } else {
                    map_err_domain()
                };
                // Only apply fallible suffix if this specific type has the method as fallible.
                // Use type_fallible_methods: (Type, method) set for precision.
                if ctx.type_fallible_methods.contains(&(bare.clone(), method.to_string())) {
                    return suffix.to_string();
                }
                // If the method is ONLY fallible (not ambiguous), apply it.
                if !ctx.non_fallible_methods.contains(method) {
                    return suffix.to_string();
                }
                // Ambiguous and not confirmed fallible on this type: no suffix.
            }
            return String::new();
        }
        if ctx.name_to_shape.get(bare.as_str()) == Some(&Shape::Trait)
            || ctx.name_to_shape.get(ty.as_str()) == Some(&Shape::Trait)
        {
            let fallible = has_bang
                || ctx
                    .type_fallible_methods
                    .contains(&(bare.clone(), method.to_string()))
                || ctx
                    .type_fallible_methods
                    .contains(&(ty.clone(), method.to_string()));
            return if fallible {
                ".await?".to_string()
            } else {
                ".await".to_string()
            };
        }
    }

    // Fluent SDK send / async fallible stubs (untyped receivers).
    if method == "send"
        || method == "send_with"
        || ctx.async_fallible_methods.contains(method)
    {
        // send!() → unwrap; bare send() keeps Result so .is_ok()/.is_err() work.
        if has_bang {
            return ".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?".to_string();
        } else {
            return ".await".to_string();
        }
    }
    // Untyped receiver: method name appears on a port trait → async_trait.
    // If a stub/struct also has the same method name (e.g. `delete`), do not
    // force await — that would break reqwest Client.delete. List elements of
    // trait objects are handled via Index + peel above (SagaStep.action).
    let is_trait_method = ctx.method_returns.keys().any(|(ty, m)| {
        m == method && ctx.name_to_shape.get(ty) == Some(&Shape::Trait)
    });
    let is_stub_or_struct_method = ctx.method_returns.keys().any(|(ty, m)| {
        m == method
            && (ctx.stub_type_crate.contains_key(ty)
                || ctx.name_to_shape.get(ty) == Some(&Shape::Struct))
    });
    if is_trait_method && !is_stub_or_struct_method {
        return if has_bang {
            ".await?".to_string()
        } else {
            ".await".to_string()
        };
    }
    // Sync Res! stub methods: map any Error into DomainError.
    // Only apply when the receiver is NOT a chained Call — intermediate methods
    // in builder chains (returning Self) should not get fallible suffixes even if
    // a method of the same name is fallible on a different type (Issue 5/global
    // name collision: e.g. gix.prefix() is Res! but S3 builder.prefix() is not).
    // Also skip if the method is ambiguous — exists as both fallible and non-fallible
    // across different stub types (e.g. gix Id.detach() is non-fallible but
    // Pathspec.detach() is fallible).
    let recv_is_chain = matches!(recv, Expr::Call(_));
    let is_ambiguous = ctx.non_fallible_methods.contains(method);
    if ctx.fallible_methods.contains(method) && !recv_is_chain && !is_ambiguous {
        let own = should_own_str_result(ctx, recv_type_name.as_deref(), method);
        return if own {
            map_err_domain_own_str()
        } else {
            map_err_domain()
        }
        .to_string();
    }
    // Terminal builder `.build!()` is fallible (BuildError) even on chains.
    if has_bang && method == "build" {
        return map_err_domain().to_string();
    }
    // Fallback: if the method has a bang (!) and nothing else matched,
    // treat it as an async fallible call (common for SDK methods like collect!,
    // execute!, etc. on receivers whose type isn't in our stub system).
    if has_bang {
        return ".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?".to_string();
    }
    String::new()
}

/// `Box<dyn SagaStep + Send + Sync>` / `Arc<dyn SnsClient + Send + Sync>` → trait name
fn peel_dyn_trait_name(ty: &str) -> Option<String> {
    let t = ty.trim();
    let after_dyn = if let Some(rest) = t.strip_prefix("Box<dyn ") {
        rest
    } else if let Some(rest) = t.strip_prefix("Arc<dyn ") {
        rest
    } else if let Some(rest) = t.strip_prefix("std::sync::Arc<dyn ") {
        rest
    } else if let Some(rest) = t.strip_prefix("dyn ") {
        rest
    } else {
        return None;
    };
    let name = after_dyn.split(|c: char| c == '+' || c == '>' || c == ' ').next()?;
    if !name.is_empty() {
        Some(name.to_string())
    } else {
        None
    }
}

/// Map a VEIL `self.X` / bare env ident onto the rust adapter field name.
/// `@env(TABLE_NAME)` is `table_name`; last-segment (`table` / `name`) and
/// the original `TABLE_NAME` still resolve to that field.
fn resolve_self_field_name(ctx: &GenCtx, field: &str) -> Option<String> {
    let snake = to_snake(field);
    let lower = field.to_ascii_lowercase();
    if ctx.self_field_types.contains_key(&snake) {
        return Some(snake);
    }
    if ctx.self_field_types.contains_key(&lower) {
        return Some(lower);
    }
    let mut best: Option<String> = None;
    let consider = |known: &str, needle: &str, best: &mut Option<String>| {
        if known == needle || known.rsplit('_').next() == Some(needle) {
            match best {
                None => *best = Some(known.to_string()),
                Some(b) if known.len() > b.len() => *best = Some(known.to_string()),
                _ => {}
            }
        }
    };
    for known in ctx
        .self_field_types
        .keys()
        .chain(ctx.self_fields.iter())
    {
        consider(known, &snake, &mut best);
        consider(known, &lower, &mut best);
    }
    best.filter(|b| ctx.self_fields.contains(b) || ctx.self_field_types.contains_key(b))
}

/// `Vec<Box<dyn T + …>>` / `&[Box<dyn T + …>]` → element type string
fn extract_vec_elem(ty: &str) -> Option<String> {
    let t = ty.trim();
    if let Some(inner) = t.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
        return Some(inner.trim().to_string());
    }
    if let Some(inner) = t.strip_prefix("&[").and_then(|s| s.strip_suffix(']')) {
        return Some(inner.trim().to_string());
    }
    if let Some(inner) = t.strip_prefix("&mut [").and_then(|s| s.strip_suffix(']')) {
        return Some(inner.trim().to_string());
    }
    None
}

fn extract_box_dyn_trait(ty: &str) -> Option<String> {
    if let Some(elem) = extract_vec_elem(ty) {
        return peel_dyn_trait_name(&elem).or(Some(elem));
    }
    peel_dyn_trait_name(ty)
}

/// Rust method/path segment for a call: keep PascalCase for enum variants /
/// associated constructors (`AttributeValue::S`); snake_case for normal methods.
/// Strip VEIL fallible/query suffixes (`!` / `?`) — those are typecheck sugar only.
fn rust_method_name(method: &str) -> String {
    let method = method.trim_end_matches(['!', '?']);
    if method
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        method.to_string()
    } else {
        to_snake(method)
    }
}

/// Build a `serde_json::json!` object for a message with a `"type"` tag plus
/// its named fields — the wire form for a JSON envelope payload.
fn json_message(name: &str, fields: &[(String, Expr)], ctx: &GenCtx) -> String {
    let mut parts = vec![format!("\"type\": \"{}\"", name)];
    for (k, v) in fields {
        parts.push(format!("\"{}\": {}", k, to_json_arg(v, ctx)));
    }
    format!("serde_json::json!({{ {} }})", parts.join(", "))
}

/// Build a JSON envelope for a cross-boundary call routed through a routing
/// trait: `{ "target": T, "method": m, "args": [ ... ] }`. Positional args are
/// rendered as JSON values so the receiving side can decode them.
fn json_envelope(target: &str, method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    let arg_vals = args.iter().map(|a| to_json_arg(a, ctx)).collect::<Vec<_>>().join(", ");
    format!(
        "serde_json::json!({{ \"target\": \"{}\", \"method\": \"{}\", \"args\": [{}] }})",
        target, method, arg_vals
    )
}

/// Render call args, cloning value-bearing locals/state so passing them into a
/// by-value parameter doesn't move them out of the caller. Skips the routing
/// reference and Copy scalars (which don't move).
fn clone_args(args: &[Expr], ctx: &GenCtx) -> String {
    args.iter()
        .map(|a| match a {
            Expr::Ident(n) if ctx.state_locals.contains(n.as_str()) => format!("state[\"{}\"].clone()", n),
            // The routing reference and Copy scalars are passed as-is.
            Expr::Ident(n) if !ctx.routing_ref.is_empty() && *n == ctx.routing_ref => n.clone(),
            Expr::Ident(n) if is_copy_local(n, ctx) => n.clone(),
            Expr::Ident(n) if is_ref_local(n, ctx) => n.clone(),
            // sqlx Executor is implemented for `&Pool`, not `Pool`.
            Expr::Ident(n) if n == "pool" => "&self.pool".to_string(),
            Expr::Ident(n) if ctx.is_local(n) => format!("{}.clone()", n),
            Expr::FieldAccess(base, field)
                if field == "pool"
                    && matches!(base.as_ref(), Expr::Ident(n) if n == "self") =>
            {
                "&self.pool".to_string()
            }
            _ => expr_to_rust(a, ctx),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Like `clone_args` but applies method-specific argument shaping (e.g. reqwest
/// `basic_auth` takes `Option` password).
fn clone_args_for_method(method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    clone_args_for_typed_method(None, method, args, ctx)
}

/// Clone/ref args for a method call, with optional receiver type for ref-param resolution.
fn clone_args_for_typed_method(recv_type: Option<&str>, method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    let method = method.trim_end_matches(['!', '?']);

    // Check ref_params for this specific (type, method) combination.
    // If found, emit &arg for ref positions instead of arg.clone().
    if let Some(type_name) = recv_type {
        if let Some(ref_flags) = ctx.ref_params.get(&(type_name.to_string(), method.to_string())) {
            return args.iter().enumerate().map(|(i, a)| {
                let is_ref = ref_flags.get(i).copied().unwrap_or(false);
                if is_ref {
                    let s = expr_to_rust(a, ctx);
                    if s.starts_with('&') {
                        s
                    } else if matches!(a, Expr::Ident(n) if ctx.is_local(n)) {
                        // Deref to &str for String locals — avoids &String which
                        // doesn't satisfy generic bounds like TryInto<FullName>.
                        format!("&*{s}")
                    } else if let Expr::StringLit(lit) = a {
                        // ref params expecting &str: emit bare string literal
                        format!("\"{}\"", lit.replace('\\', "\\\\").replace('"', "\\\""))
                    } else {
                        format!("&{s}")
                    }
                } else {
                    // Normal clone behavior for non-ref params
                    match a {
                        Expr::Ident(n) if ctx.is_local(n) && !is_copy_local(n, ctx) => {
                            format!("{}.clone()", n)
                        }
                        _ => expr_to_rust(a, ctx),
                    }
                }
            }).collect::<Vec<_>>().join(", ");
        }
    }
    // str::starts_with / contains / ends_with / replace take Pattern / &str —
    // string lits as &str, not owned String (Pattern not implemented for String).
    if matches!(
        method,
        "starts_with"
            | "contains"
            | "ends_with"
            | "strip_prefix"
            | "strip_suffix"
            | "replace"
            | "replacen"
            | "split"
    ) {
        return args
            .iter()
            .map(|a| match a {
                Expr::StringLit(s) => {
                    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                }
                _ => {
                    let s = expr_to_rust(a, ctx);
                    // Owned String locals: borrow for Pattern / &str
                    if matches!(a, Expr::Ident(_)) {
                        format!("&{s}")
                    } else if s.starts_with('&') {
                        s
                    } else {
                        format!("&({s})")
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
    }
    // Option<&str>.unwrap_or("") — keep bare &str, not String
    if method == "unwrap_or" && args.len() == 1 {
        if let Expr::StringLit(s) = &args[0] {
            return format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        }
    }
    if method == "basic_auth" && args.len() >= 2 {
        let user = clone_args(&args[..1], ctx);
        let pass = expr_to_rust(&args[1], ctx);
        // reqwest: basic_auth(user, Option<password>)
        return format!("{user}, Some({pass})");
    }
    // sqlx bind: Uuid needs the `uuid` feature; bind as text to stay feature-light.
    if method == "bind" && args.len() == 1 {
        if let Expr::Ident(n) = &args[0] {
            if ctx.local_type(n) == Some("Uuid")
                || n == "id"
                || n.ends_with("_id")
                || n.ends_with("Id")
            {
                return format!("{n}.to_string()");
            }
        }
        if let Expr::FieldAccess(base, field) = &args[0] {
            let f = to_snake(field);
            if f == "id" || f.ends_with("_id") {
                let b = expr_to_rust(base, ctx);
                // self.x.clone().id → already cloned base
                if b.ends_with(".clone()") {
                    return format!("{b}.{f}.to_string()");
                }
                return format!("{b}.{f}.to_string()");
            }
        }
    }
    let param_tys = param_types_for(recv_type, method, ctx);
    args.iter()
        .enumerate()
        .map(|(i, a)| arg_to_rust(a, param_tys.get(i).map(|s| s.as_str()), ctx))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Check if an expression is rooted in a local whose type is serde_json::Value.
/// Handles arbitrary depth of FieldAccess chains.
fn is_json_rooted_expr(expr: &Expr, ctx: &GenCtx) -> bool {
    match expr {
        Expr::Ident(name) => {
            ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value")
        }
        Expr::FieldAccess(base, _) => is_json_rooted_expr(base, ctx),
        _ => false,
    }
}

/// A local whose inferred type is a Copy scalar (int/bool/float) — no clone.
fn is_copy_local(name: &str, ctx: &GenCtx) -> bool {
    matches!(
        ctx.local_type(name),
        Some("i64") | Some("i32") | Some("u64") | Some("u32")
            | Some("usize") | Some("isize") | Some("f64") | Some("f32") | Some("bool")
    )
}

/// Locals that are already references / trait objects / slices — `.clone()` is a no-op.
fn is_ref_local(name: &str, ctx: &GenCtx) -> bool {
    let Some(ty) = ctx.local_type(name) else {
        return false;
    };
    ty.starts_with('&')
        || ty.contains("dyn ")
        || ty.starts_with('[')
        || ty.contains("&[")
}

fn is_hashmap_param(ty: &str) -> bool {
    ty.contains("HashMap") || ty.starts_with("Map<")
}

fn is_option_param(ty: &str) -> bool {
    ty.starts_with("Option<") || ty.starts_with("Opt<")
}

/// Peel `Arc<dyn Port + Send + Sync>` / `Option<Foo>` down to the type name
/// used as a `method_params` key.
fn peel_type_key(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.split("dyn ").nth(1) {
        return rest
            .split(|c: char| c == '+' || c == '<' || c == '>' || c == ',')
            .next()
            .unwrap_or(rest)
            .trim()
            .to_string();
    }
    s.split('<')
        .next()
        .unwrap_or(s)
        .trim()
        .trim_start_matches('&')
        .to_string()
}

fn param_types_for(recv: Option<&str>, method: &str, ctx: &GenCtx) -> Vec<String> {
    let bare = method.trim_end_matches(['!', '?']).to_string();
    let mut keys: Vec<(String, String)> = match recv {
        Some(r) => {
            let snake = to_snake(r);
            vec![
                (r.to_string(), method.to_string()),
                (r.to_string(), bare.clone()),
                (snake.clone(), method.to_string()),
                (snake, bare.clone()),
            ]
        }
        None => Vec::new(),
    };
    if let Some(r) = recv {
        // @dep field → trait (`sns_client` → `SnsClient`)
        if let Some((trait_name, _)) = ctx
            .dep_fields
            .iter()
            .find(|(_, f)| f.as_str() == r || f.as_str() == to_snake(r))
        {
            keys.push((trait_name.clone(), method.to_string()));
            keys.push((trait_name.clone(), bare.clone()));
            keys.push((to_snake(trait_name), method.to_string()));
            keys.push((to_snake(trait_name), bare.clone()));
        }
        // Adapter field rust type (`Arc<dyn SnsClient + …>`) → trait key
        if let Some(fty) = ctx
            .self_field_types
            .get(r)
            .or_else(|| ctx.self_field_types.get(&to_snake(r)))
        {
            let peeled = peel_type_key(fty);
            if !peeled.is_empty() {
                keys.push((peeled.clone(), method.to_string()));
                keys.push((peeled.clone(), bare.clone()));
                keys.push((to_snake(&peeled), method.to_string()));
                keys.push((to_snake(&peeled), bare.clone()));
            }
        }
    }
    for k in &keys {
        if let Some(p) = ctx.method_params.get(k) {
            return p.clone();
        }
    }
    // Prefer a unique Map-bearing signature for this method when the
    // receiver key missed (dep field vs stub fluent of the same name).
    let map_hits: Vec<&Vec<String>> = ctx
        .method_params
        .iter()
        .filter(|((_, m), tys)| {
            (*m == method || *m == bare)
                && tys
                    .iter()
                    .any(|t| is_hashmap_param(t))
        })
        .map(|(_, v)| v)
        .collect();
    if map_hits.len() == 1 {
        return map_hits[0].clone();
    }
    if let Some(first) = map_hits.first() {
        if map_hits.iter().all(|h| *h == *first) {
            return (*first).clone();
        }
    }
    let hits: Vec<&Vec<String>> = ctx
        .method_params
        .iter()
        .filter(|((_, m), _)| *m == method || *m == bare)
        .map(|(_, v)| v)
        .collect();
    if hits.len() == 1 {
        return hits[0].clone();
    }
    if let Some(first) = hits.first() {
        if hits.iter().all(|h| *h == *first) {
            return (*first).clone();
        }
    }
    Vec::new()
}

fn map_literal_to_hashmap(fields: &[(String, Expr)], ctx: &GenCtx) -> String {
    if fields.is_empty() {
        return "std::collections::HashMap::new()".to_string();
    }
    let inserts: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            let val = expr_to_rust(v, ctx);
            format!("__m.insert(\"{k}\".to_string(), {val})")
        })
        .collect();
    format!(
        "{{ let mut __m = std::collections::HashMap::new(); {}; __m }}",
        inserts.join("; ")
    )
}

fn arg_looks_optional(arg: &Expr, rust: &str, ctx: &GenCtx) -> bool {
    rust.starts_with("Some(")
        || rust == "None"
        || rust.starts_with("None::<")
        || match arg {
            Expr::Ident(n) => ctx
                .local_type(n)
                .map(|t| t.starts_with("Option<") || t.starts_with("Opt<"))
                .unwrap_or(false),
            _ => false,
        }
}

fn arg_to_rust(arg: &Expr, param_ty: Option<&str>, ctx: &GenCtx) -> String {
    let mut rust = if let (Some(ty), Expr::StructLit(name, fields)) = (param_ty, arg) {
        if name.is_empty() && is_hashmap_param(ty) {
            map_literal_to_hashmap(fields, ctx)
        } else {
            expr_to_rust(arg, ctx)
        }
    } else {
        match arg {
            Expr::Ident(n) if ctx.state_locals.contains(n.as_str()) => {
                format!("state[\"{n}\"].clone()")
            }
            Expr::Ident(n) if !ctx.routing_ref.is_empty() && *n == ctx.routing_ref => n.clone(),
            Expr::Ident(n) if is_copy_local(n, ctx) => n.clone(),
            Expr::Ident(n) if is_ref_local(n, ctx) => n.clone(),
            Expr::Ident(n) if n == "pool" => "&self.pool".to_string(),
            Expr::Ident(n) if ctx.is_local(n) => format!("{n}.clone()"),
            Expr::FieldAccess(base, field)
                if field == "pool" && matches!(base.as_ref(), Expr::Ident(n) if n == "self") =>
            {
                "&self.pool".to_string()
            }
            _ => expr_to_rust(arg, ctx),
        }
    };
    if let Some(ty) = param_ty {
        if is_option_param(ty) && !arg_looks_optional(arg, &rust, ctx) {
            rust = format!("Some({rust})");
        }
    }
    rust
}

/// Receiver name used to look up port/stub param types.
/// Ident and `self.field` / `deps.field` last segments all count.
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

fn call_args_to_rust(call: &CallExpr, ctx: &GenCtx) -> String {
    let recv_owned = call_recv_lookup_name(call);
    let recv = recv_owned.as_deref();
    let tys = param_types_for(recv, &call.method, ctx);
    call.args
        .iter()
        .enumerate()
        .map(|(i, a)| arg_to_rust(a, tys.get(i).map(|s| s.as_str()), ctx))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Translate a Call expression with shape-aware name resolution.
fn translate_call(call: &CallExpr, ctx: &GenCtx) -> String {
    let args_str = call_args_to_rust(call, ctx);

    // Built-in List methods: `.get(i)` → indexing (`[i as usize]`), `.len()` →
    // `.len() as i64`. The receiver/target is the list expression.
    // Only treat `.get` as slice index when the arg is index-like (int lit /
    // int-typed local) OR the receiver is known to be a Vec/slice (saga
    // coordinators: `steps: List<SagaStep>` → `&[Box<dyn …>]`). `client.get(url)`
    // and `map.get(key)` must stay method calls.
    let list_base = if let Some(recv) = &call.receiver {
        Some(expr_to_rust(recv, ctx))
    } else if !call.target.is_empty()
        && !ctx.is_trait_target(&call.target)
        && (call.method == "get" || call.method == "len")
        && ctx.local_type(&call.target) != Some("serde_json::Value")
    {
        Some(call.target.clone())
    } else {
        None
    };
    if let Some(base) = list_base {
        if call.method == "get" && call.args.len() == 1 {
            // String args (HashMap key lookup) stay as .get("key") — fall through.
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
            // Receiver known as Vec / slice → index even if local types lag
            // (e.g. `mut i = upto` before Ident inference is wired).
            let base_is_list = if !call.target.is_empty() {
                ctx.local_type(&call.target)
                    .map(|t| {
                        t.starts_with("Vec<")
                            || t.starts_with("&[")
                            || t.starts_with("&mut [")
                    })
                    .unwrap_or(false)
            } else if let Some(recv) = &call.receiver {
                if let Expr::Ident(n) = recv.as_ref() {
                    ctx.local_type(n)
                        .map(|t| {
                            t.starts_with("Vec<")
                                || t.starts_with("&[")
                                || t.starts_with("&mut [")
                        })
                        .unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            };
            if !is_string_arg && (arg_is_index_like || base_is_list) {
                let idx = expr_to_rust(&call.args[0], ctx);
                return format!("{}[({}) as usize]", base, idx);
            }
        }
        if call.method == "len" && call.args.is_empty() {
            return format!("({}.len() as i64)", base);
        }
    }

    // Chained method call: `<receiver>.method(args)` (e.g. `.collect()` in
    // `items.map(f).collect()`). The receiver carries the left side of the chain.
    if let Some(recv) = &call.receiver {
        let mut recv_str = expr_to_rust(recv, ctx);

        // Auto-unwrap Option<T> locals for method calls: when the receiver is a
        // local typed as Option<T> and the method is NOT an Option method, unwrap
        // first so that domain-type methods can be called directly.
        if let Expr::Ident(name) = recv.as_ref() {
            if let Some(ty) = ctx.local_type(name) {
                if ty.starts_with("Option<") {
                    let bare_method = call.method.trim_end_matches(['!', '?']);
                    let option_methods = [
                        "is_some", "is_none", "unwrap", "unwrap_or", "unwrap_or_else",
                        "unwrap_or_default", "map", "and_then", "or_else", "ok_or",
                        "ok_or_else", "as_ref", "as_mut", "take", "replace", "clone",
                        "expect", "filter", "flatten", "zip",
                    ];
                    if !option_methods.contains(&bare_method) {
                        recv_str = format!(
                            "{}.clone().ok_or(DomainError::NotFound)?",
                            recv_str
                        );
                    } else {
                        // Consuming Option methods (and_then, map, unwrap, filter, etc.)
                        // move self — clone to allow reuse of the local variable.
                        let non_consuming = ["is_some", "is_none", "as_ref", "as_mut", "clone"];
                        if !non_consuming.contains(&bare_method) {
                            recv_str = format!("{}.clone()", recv_str);
                        }
                    }
                }
            }
        }

        let bare_conv = call.method.trim_end_matches(['!', '?']);
        if matches!(bare_conv, "to_str" | "as_str" | "to_string") && call.args.is_empty() {
            let recv_is_string = matches!(recv.as_ref(), Expr::Ident(n) if ctx.local_type(n) == Some("String"));
            if !recv_is_string {
                return format!("String::from_utf8_lossy({recv_str}.as_ref()).to_string()");
            }
        }
        // Stub/`Str` as_ref is a bytes view in Rust (`&[u8]`). Honor VEIL Str.
        if matches!(bare_conv, "as_ref") && call.args.is_empty() && should_decode_as_ref_to_str(recv, ctx)
        {
            return format!("String::from_utf8_lossy({recv_str}.as_ref()).to_string()");
        }
        if matches!(bare_conv, "as_bytes" | "to_bytes" | "into_bytes") && call.args.is_empty() {
            return format!("{recv_str}.as_ref().to_vec()");
        }

        // Phase 2, Issue 1: Redundant .unwrap() elision.
        // When the receiver is itself a Call whose codegen already unwraps the value
        // (as_s, as_n → .map_err()?  /  get("lit") → .ok_or_else()?), a following
        // .unwrap() is redundant and would error (String/&AV has no .unwrap()).
        if (call.method == "unwrap" || call.method == "unwrap!") && call.args.is_empty() {
            if let Expr::Call(inner_call) = recv.as_ref() {
                let inner_bare = inner_call.method.trim_end_matches(['!', '?']);
                // as_s / as_n already produce a fully-unwrapped String
                if inner_bare == "as_s" || inner_bare == "as_n" || (inner_bare.starts_with("as_") && inner_bare != "as_str") {
                    return recv_str;
                }
                // .get("key") already produces .ok_or_else(...)? — value is extracted
                if inner_bare == "get" && inner_call.args.len() == 1 {
                    if matches!(&inner_call.args[0], Expr::StringLit(_)) {
                        return recv_str;
                    }
                }
            }
            // Also catch: recv_str ends with `)?` or `.unwrap()` — redundant unwrap
            let trimmed = recv_str.trim();
            if trimmed.ends_with(")?") || trimmed.ends_with(".unwrap()") {
                return recv_str;
            }
        }

        // Map/HashMap .get("lit") → &str key (not String) on any receiver chain.
        // Match local-target lowering: unwrap Option for immediate .as_s() chains.
        if call.method == "get" && call.args.len() == 1 {
            if let Expr::StringLit(key) = &call.args[0] {
                // Issue 6: never panic on missing map keys in adapter bodies.
                return format!(
                    "{}.get(\"{}\").ok_or_else(|| DomainError::External(\"missing {}\".into()))?",
                    recv_str, key, key
                );
            }
        }
        // serde_json::Value::as_str → Option<String> (owned) for assigns/unwrap.
        if call.method == "as_str" && call.args.is_empty() {
            return format!("{}.as_str().map(|s| s.to_string())", recv_str);
        }
        // Stub `Res!<Str>` getters (`as_s` / `as_n` / typed as_*): Rust is
        // usually `Result<&str, E>` with E: Debug + !Display. Own a String.
        if call.args.is_empty() {
            let recv_ty = infer_expr_type(recv, ctx);
            if should_own_str_result(ctx, recv_ty.as_deref(), &call.method) {
                let m = method_bare(&call.method);
                return format!("{recv_str}.{m}(){}", map_err_domain_own_str());
            }
        }
        // A trait method invoked on a chained receiver is async + fallible.
        let suffix = receiver_call_suffix(recv, &call.method, ctx);
        let m = rust_method_name(&call.method);
        let bare_m = call.method.trim_end_matches(['!', '?']);
        // .trim() on a String returns &str — own it for return/assign contexts.
        if bare_m == "trim" && call.args.is_empty() {
            return format!("{}.trim().to_string()", recv_str);
        }
        if (bare_m == "unwrap_or" || bare_m == "unwrap_or_else") && call.args.len() == 1 {
            if let Expr::StringLit(s) = &call.args[0] {
                let lit = s.replace('\\', "\\\\").replace('"', "\\\"");
                // Option<String> (after .map(|s| s.to_string()) / .clone() / .as_str().map(...)
                // / .and_then(|c| c.field)):
                // need owned default. Option<&str> (AWS getters): bare &str.
                // VEIL Str always maps to Rust String, so .and_then() / .map()
                // chains on domain types produce Option<String>. Only explicit
                // AWS getter patterns (handled via as_str() → map) stay &str.
                let owned_default = recv_str.contains("to_string()")
                    || recv_str.contains("as_str().map")
                    || recv_str.ends_with(".clone()")
                    || recv_str.contains(".and_then(")
                    || recv_str.contains(".map(");
                if owned_default {
                    return format!("{}.{m}(\"{lit}\".to_string()){suffix}", recv_str);
                }
                return format!("{}.{m}(\"{lit}\"){suffix}", recv_str);
            }
        }
        // Phase 2, Issue 3: S3 .body() takes ByteStream, not Vec<u8>.
        // Append .into() when the arg is a local typed as Vec<u8>/Bytes.
        if bare_m == "body" && call.args.len() == 1 {
            if let Expr::Ident(name) = &call.args[0] {
                let ty = ctx.local_type(name).unwrap_or("");
                if ty == "Vec<u8>" || ty.contains("Bytes") || ty.contains("Vec<u8>") {
                    return format!("{}.body({}.into()){}", recv_str, name, suffix);
                }
            }
        }
        // Phase 2, Issue 4: DDB .limit() takes i32, VEIL Int is i64.
        // Insert `as i32` cast for the argument.
        if bare_m == "limit" && call.args.len() == 1 {
            let arg = expr_to_rust(&call.args[0], ctx);
            // Only cast if the arg could be i64 (ident or int lit, not already cast)
            if !arg.contains("as i32") {
                return format!("{}.limit(({}) as i32){}", recv_str, arg, suffix);
            }
        }
        // Look up param types by receiver *name* (port/dep field), not the
        // inferred Rust local type. `local_type("sns_client")` is None; the
        // port is registered as `(sns_client, publish)`. Falling back to
        // "any method named publish" collides with stub fluent `publish()`.
        let recv_lookup = match recv.as_ref() {
            Expr::Ident(name) => Some(name.as_str()),
            Expr::FieldAccess(_, field) => Some(field.as_str()),
            _ => None,
        };
        return format!(
            "{}.{}({}){}",
            recv_str,
            m,
            clone_args_for_typed_method(recv_lookup, &call.method, &call.args, ctx),
            suffix
        );
    }

    // Trait-shaped target → deps.<field>.method(args).await?
    // Field name comes from dep_fields (shared with harness / Deps struct).
    if ctx.is_trait_target(&call.target) {
        let dep_name = ctx.deps_field_for(&call.target);
        let method = if call.method.is_empty() { "call" } else { &call.method };
        // Desugared routing-port calls (layer statement sugar) carry a StructLit
        // payload; build a JSON message tagged with its type.
        let final_args = if call.sugar.is_some() {
            match call.args.first() {
                Some(Expr::StructLit(name, fields)) => json_message(name, fields, ctx),
                Some(Expr::Ident(evt)) => format!("serde_json::json!({{ \"type\": \"{}\" }})", evt),
                _ => json_envelope(&call.target, method, &call.args, ctx),
            }
        } else {
            // Direct routing-trait call — clone args to avoid move issues.
            // Auto-unwrap Option<T> only when the port param is T (not Option<T>).
            let method_key = method.trim_end_matches(['!', '?']);
            let param_tys = param_types_for(Some(call.target.as_str()), method_key, ctx);
            call.args
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let expected = param_tys.get(i).map(|s| s.as_str());
                    let s = arg_to_rust(a, expected, ctx);
                    match a {
                        Expr::Ident(name) if ctx.local_type(name) == Some("serde_json::Value") => {
                            format!("{}.clone()", name)
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
                                format!("{}.clone()", name)
                            } else {
                                format!("{}.clone().ok_or(DomainError::NotFound)?", name)
                            }
                        }
                        _ => s,
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        // Routing traits use `routing_ref` (`deps.<trait>` in a flow, injected
        // param inside a step impl); other trait deps come from `deps`.
        if ctx.routing_traits.contains(&call.target) {
            let rref = if ctx.routing_ref.is_empty() {
                format!("deps.{}", dep_name)
            } else {
                ctx.routing_ref.clone()
            };
            let bare = to_snake(method);
            let call_expr = format!("{}.{}({}).await?", rref, bare, final_args);
            // Typed bus decode: when sugar carries a message type with a known
            // domain return, deserialize instead of leaving serde_json::Value.
            if matches!(bare.as_str(), "invoke" | "request") {
                if let Some(msg) = bus_message_name_from_args(&call.args) {
                    if let Some(ret) = ctx.bus_returns.get(&msg) {
                        // Only decode types this crate can name (local domain /
                        // primitives). Cross-context domain types (e.g. tools
                        // invoking storage CreateRepo → Repo) stay as Value.
                        if bus_return_type_in_scope(ctx, ret) {
                            return format!(
                                "serde_json::from_value::<{ret}>({call_expr})\
                                 .map_err(|e| DomainError::External(e.to_string()))?"
                            );
                        }
                    }
                }
            }
            return call_expr;
        }
        // Bang on ports means fallible/async (Result), not "unwrap Opt".
        // Keep Option so callers can use .is_some() / .is_none() / .unwrap().
        let method_key = method.trim_end_matches(['!', '?']);
        // Port methods that return non-Result types (e.g. Bool, plain Str)
        // should NOT have `?` appended — they are async but not fallible.
        // However, bang (`!`) on the call site always means fallible — the
        // method wraps its return in Result even if the inner type is `()`.
        let has_bang = method.ends_with('!');
        let ret_type = ctx.return_type_of(&call.target, method)
            .or_else(|| {
                // Also try the PascalCase trait name via dep_fields reverse lookup
                ctx.dep_fields.iter()
                    .find(|(_, v)| *v == &call.target)
                    .and_then(|(trait_name, _)| ctx.return_type_of(trait_name, method))
            });
        let is_fallible = if has_bang {
            true // Bang always means Result-wrapped (fallible)
        } else {
            match ret_type {
                Some("bool") | Some("Bool") | Some("i64") | Some("f64")
                | Some("String") | Some("()") | Some("") => false,
                Some(t) if t.starts_with("Option<") || t.starts_with("Opt<") => false,
                _ => true,
            }
        };
        let suffix = if is_fallible { ".await?" } else { ".await" };
        return format!(
            "deps.{}.{}({}){}",
            dep_name,
            to_snake(method_key),
            final_args,
            suffix,
        );
    }

    // Envelope routing: cross-boundary calls (struct construction, foreign
    // methods, etc.) go through the primary routing trait with a typed JSON
    // envelope — the caller crate cannot see the target's concrete types.
    // Language primitives (Json, Map, Dt, etc.) are excluded — they resolve locally.
    // Locals with known types (esp. serde_json::Value) are also excluded —
    // they are calling methods on data, not cross-boundary invocations.
    let is_lang_target = matches!(
        call.target.as_str(),
        "Dt" | "DateTime" | "Uuid" | "Map" | "List" | "Opt" | "Json" | "Env" | "Str" | "Id" | "UUID"
    );
    let is_typed_local = ctx.is_local(&call.target) && ctx.local_type(&call.target).is_some();
    if ctx.envelope_routing && !is_lang_target && !is_typed_local
        && !ctx.stub_pkg_crate.contains_key(&call.target)
        && (ctx.is_struct_target(&call.target) || ctx.is_local(&call.target) || !call.method.is_empty()) {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        let rref = if ctx.routing_ref.is_empty() {
            "deps".to_string() // should not happen when envelope_routing is set
        } else {
            ctx.routing_ref.clone()
        };
        return format!(
            "{}.invoke({}).await?",
            rref,
            json_envelope(&call.target, method, &call.args, ctx)
        );
    }

    // Language primitives win over stub names (e.g. gix.stub `struct Id`,
    // axum.stub `Json` — IR Json is not axum::Json).
    if !call.method.is_empty() {
        let lang = match (call.target.as_str(), call.method.as_str()) {
            ("Id", "new") | ("Id", "new_v4") | ("UUID", "new") | ("UUID", "new_v4") | ("Uuid", "new")
                => Some("Uuid::new_v4()".to_string()),
            ("Dt", "now") => Some("Utc::now()".to_string()),
            ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601")
                => Some(now_iso8601_rust()),
            ("Json", "parse") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!("serde_json::from_str::<_>(&{})?", arg))
            }
            ("Json", "stringify") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!("serde_json::to_string(&{})?", arg))
            }
            ("Json", "null") => Some("serde_json::Value::Null".to_string()),
            ("Json", "object") => Some("serde_json::Value::Object(serde_json::Map::new())".to_string()),
            ("Json", "array") => Some("serde_json::Value::Array(Vec::new())".to_string()),
            _ => None,
        };
        if let Some(result) = lang {
            return result;
        }
    }

    // Built-in type-level method translations.
    // These are VEIL's short type names with associated methods that map
    // to Rust idioms. Language primitives always win over stub types that
    // happen to share a name (e.g. sqlx's `Map` must not steal `Map.new()`).
    if !call.method.is_empty() {
        let lang_leaf = lang_type_leaf(&call.target);
        let is_lang_primitive = matches!(
            lang_leaf,
            "Dt" | "DateTime" | "Uuid" | "Map" | "List" | "Opt" | "Json" | "Env" | "Str" | "Id" | "Process"
                | "Blob" | "Bytes"
        );
        if is_lang_primitive || !ctx.is_struct_target(&call.target) {
            let method_key = call.method.trim_end_matches(['!', '?']);
            let translated = match (lang_leaf, method_key) {
                ("Dt", "now") => Some("Utc::now()".to_string()),
                ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601")
                    if call.args.is_empty() =>
                {
                    Some(now_iso8601_rust())
                }
                ("Uuid", "new_v4") | ("Id", "new_v4") => Some("Uuid::new_v4()".to_string()),
                ("Map", "new") => Some("HashMap::new()".to_string()),
                ("List", "new") => Some("Vec::new()".to_string()),
                ("Opt", "empty") | ("Opt", "none") => Some("None".to_string()),
                ("Opt", "some") | ("Opt", "of") if call.args.len() == 1 => {
                    Some(format!("Some({})", expr_to_rust(&call.args[0], ctx)))
                }
                ("Env", "get_or") if call.args.len() == 2 => {
                    let var = expr_to_rust(&call.args[0], ctx);
                    // StringLit already becomes `"…".to_string()` — do not double.
                    let default = match &call.args[1] {
                        Expr::StringLit(s) => format!("\"{}\".to_string()", s),
                        other => {
                            let d = expr_to_rust(other, ctx);
                            if d.ends_with(".to_string()") {
                                d
                            } else {
                                format!("{d}.to_string()")
                            }
                        }
                    };
                    Some(format!(
                        "std::env::var({}).unwrap_or_else(|_| {})",
                        var, default
                    ))
                }
                ("Env", "get_opt") if call.args.len() == 1 => {
                    let var = expr_to_rust(&call.args[0], ctx);
                    Some(format!("std::env::var({}).ok()", var))
                }
                ("Json", "parse") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!("serde_json::from_str(&{})?", arg))
                }
                ("Json", "stringify") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!("serde_json::to_string(&{})?", arg))
                }
                ("Json", "null") => Some("serde_json::Value::Null".to_string()),
                ("Json", "object") => Some("serde_json::Value::Object(serde_json::Map::new())".to_string()),
                ("Json", "array") => Some("serde_json::Value::Array(Vec::new())".to_string()),
                ("Str", "from_bytes") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!("String::from_utf8({})?", arg))
                }
                // Host process execution (language primitive — not a product facade).
                // Always returns a detail String; non-zero exit → "prog failed: …" (no hard Err)
                // so provision/job steps can record failure without 502. Spawn I/O errors still Err.
                ("Process", "run") if call.args.len() == 3 => {
                    let prog = expr_to_rust(&call.args[0], ctx);
                    let args = expr_to_rust(&call.args[1], ctx);
                    let cwd = expr_to_rust(&call.args[2], ctx);
                    let hard = call.method.ends_with('!');
                    if hard {
                        Some(format!(
                            "{{ let __prog: String = ({prog}).to_string(); let __args: String = ({args}).to_string(); let __cwd: String = ({cwd}).to_string(); let __argv: Vec<&str> = __args.split_whitespace().collect(); let __out = std::process::Command::new(&__prog).args(&__argv).current_dir(&__cwd).output().map_err(|e| DomainError::External(format!(\"{{e:?}}\")))?; if !__out.status.success() {{ let __err = String::from_utf8_lossy(&__out.stderr); let __tail: String = __err.chars().rev().take(2000).collect::<String>().chars().rev().collect(); return Err(DomainError::External(format!(\"{{}} failed: {{}}\", __prog, __tail))); }} format!(\"{{}} ok\", __prog) }}"
                        ))
                    } else {
                        Some(format!(
                            "{{ let __prog: String = ({prog}).to_string(); let __args: String = ({args}).to_string(); let __cwd: String = ({cwd}).to_string(); let __argv: Vec<&str> = __args.split_whitespace().collect(); match std::process::Command::new(&__prog).args(&__argv).current_dir(&__cwd).output() {{ Ok(__out) => {{ if __out.status.success() {{ format!(\"{{}} ok: {{}}\", __prog, String::from_utf8_lossy(&__out.stdout).chars().take(400).collect::<String>()) }} else {{ let __err = String::from_utf8_lossy(&__out.stderr); let __tail: String = __err.chars().rev().take(1200).collect::<String>().chars().rev().collect(); format!(\"{{}} failed: {{}}\", __prog, __tail) }} }}, Err(e) => format!(\"{{}} spawn failed: {{e}}\", __prog) }} }}"
                        ))
                    }
                }
                // Binary payload: use the loaded stub type path, never a bare Vec<u8>.
                ("Blob", "new") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!(
                        "{}::new({})",
                        stub_ctor_path(ctx, &call.target),
                        bytes_from_str_expr(&arg)
                    ))
                }
                ("Bytes", "from_str") | ("Bytes", "new") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(bytes_from_str_expr(&arg))
                }
                ("Str", "from_bytes") | ("Str", "from_utf8") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!("String::from_utf8_lossy(&{arg}).to_string()"))
                }
                ("Blob", "to_str") | ("Blob", "as_str") | ("Blob", "to_string")
                    if call.args.is_empty() =>
                {
                    None // handled as receiver method below
                }
                ("Blob", "from_hex") if call.args.len() == 1 => {
                    let hex_expr = expr_to_rust(&call.args[0], ctx);
                    Some(format!(
                        "{}::new({})",
                        stub_ctor_path(ctx, &call.target),
                        bytes_from_hex_expr(&hex_expr)
                    ))
                }
                ("Blob", "from_file") if call.args.len() == 1 => {
                    let path_expr = expr_to_rust(&call.args[0], ctx);
                    Some(format!(
                        "{}::new(std::fs::read(({path_expr}).as_str()).map_err(|e| DomainError::External(e.to_string()))?)",
                        stub_ctor_path(ctx, &call.target)
                    ))
                }
                _ => None,
            };
            if let Some(result) = translated {
                return result;
            }
        }
    }

    // Struct-shaped target with method "new" or empty → Type::new(args)
    // Handle dotted paths: `sqlx.Query` → prefer stub crate matching the prefix
    // so `sqlx.Query` does not resolve to an unrelated SDK type also named Query.
    let (module_prefix, effective_target) = if call.target.contains('.') {
        let mut parts = call.target.splitn(2, '.');
        let m = parts.next().unwrap_or("").to_string();
        let t = parts.next().unwrap_or(&call.target).to_string();
        (Some(m), t)
    } else {
        (None, call.target.clone())
    };
    if ctx.is_struct_target(&effective_target)
        || ctx.stub_type_crate.contains_key(&effective_target)
        || module_prefix
            .as_ref()
            .map(|m| {
                ctx.stub_type_crate.values().any(|(c, _)| {
                    c.replace('-', "_") == *m || c.as_str() == m
                })
            })
            .unwrap_or(false)
    {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        // Qualify with crate path if type is from a stub — prefer prefix match.
        let qualified = if let Some(prefix) = &module_prefix {
            // Prefer `prefix.Type` / `prefix::Type` keys (store rust_type_path).
            // A leaf-name scan misses dotted keys and can steal a same-named
            // type from another crate.
            let dotted = format!("{prefix}.{effective_target}");
            let colon = format!("{prefix}::{effective_target}");
            if let Some((crate_name, path_type)) = stub_type_parts(ctx, &dotted)
                .or_else(|| stub_type_parts(ctx, &colon))
                .or_else(|| stub_type_parts(ctx, &effective_target).filter(|(c, _)| {
                    c.replace('-', "_") == *prefix || *c == prefix.as_str()
                }))
            {
                format!("{crate_name}::{path_type}")
            } else {
                // Unloaded stub or no matching crate: keep author module path.
                format!("{}::{}", prefix, effective_target)
            }
        } else if let Some((crate_name, original_name)) = ctx.stub_type_crate.get(&effective_target) {
            // Never crate-qualify Rust built-in types (String, Vec, etc.) even if a
            // stub happens to declare a struct with the same name (e.g. gix has `struct String`).
            let is_builtin = matches!(effective_target.as_str(),
                "String" | "Vec" | "Option" | "Result" | "Box" | "Arc" | "HashMap" | "HashSet" |
                "Path" | "PathBuf" | "Bytes" | "Duration" | "Instant"
            );
            if is_builtin {
                effective_target.clone()
            } else {
                format!("{}::{}", crate_name, original_name)
            }
        } else {
            effective_target.clone()
        };
        // Clone args to avoid move issues (idents and field access like `repo.slug`)
        let cloned = call.args.iter()
            .map(|a| {
                let s = expr_to_rust(a, ctx);
                match a {
                    Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("{}.clone()", s),
                    _ => s,
                }
            }).collect::<Vec<_>>().join(", ");
        // `Type.default()` → `Type::default()` (requires Default impl from smart ctor).
        if method == "default" && call.args.is_empty() {
            return format!("{}::default()", qualified);
        }
        if method == "new" {
            // Stub constructors that map to module-level free functions.
            // e.g. crate::Query::new(sql) → crate::query(sql)
            // When the stub declares `typed_variant` and the enclosing method has a
            // domain return type → crate::query_as::<_, T>(sql) (params from stub).
            // Only when the stub says so — a lowercase crate path is not enough
            // (`aws_sdk_lambda.Blob.new` is Type::new, not crate::blob()).
            if let Some(module) = qualified.split("::").next() {
                let is_module_fn = qualified.contains("::")
                    && module.chars().next().map(|c| c.is_lowercase()).unwrap_or(false);
                let type_leaf = qualified.split("::").last().unwrap_or("new");
                if is_module_fn
                    && stub_new_is_module_free_fn(ctx, &effective_target, type_leaf)
                {
                    let fn_name = to_snake(type_leaf);
                    let raw_args = call.args.iter()
                        .map(|a| match a {
                            Expr::StringLit(s) => format!("\"{}\"", s),
                            _ => expr_to_rust(a, ctx),
                        })
                        .collect::<Vec<_>>().join(", ");

                    // Prefer explicit stub metadata; fall back to sibling `TypeAs` heuristic.
                    let typed_meta = ctx
                        .stub_typed_ctors
                        .get(&effective_target)
                        .or_else(|| ctx.stub_typed_ctors.get(type_leaf));

                    // query_as only when fetch_* on this type returns a domain row,
                    // not Opt<Str>/List<Str> (JSON payload columns use plain query +
                    // from_str). Method return type alone is not enough — find() may
                    // return Opt<Entity> while the SQL selects a text payload.
                    let fetch_ret = ctx
                        .method_returns
                        .get(&(type_leaf.to_string(), "fetch_optional".into()))
                        .or_else(|| {
                            ctx.method_returns
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
                            return format!(
                                "{module}::{typed_fn}::<{tparams}>({raw_args})"
                            );
                        }
                        // Heuristic: Query + QueryAs both registered → query_as
                        let typed_struct = format!("{type_leaf}As");
                        let has_sibling = ctx.stub_type_crate.contains_key(&typed_struct)
                            || ctx.name_to_shape.contains_key(&typed_struct);
                        if has_sibling {
                            let typed_fn_name = format!("{fn_name}_as");
                            return format!(
                                "{module}::{typed_fn_name}::<_, {domain_type}>({raw_args})"
                            );
                        }
                    }
                    // JSON-payload adapters: SELECT → query_scalar::<_, String>;
                    // INSERT/UPDATE/DELETE → plain query (has execute, no row type).
                    if fetch_is_stringish && type_leaf == "Query" {
                        let sql_is_select = call.args.first().is_some_and(|a| {
                            matches!(a, Expr::StringLit(s) if s.trim_start().to_ascii_lowercase().starts_with("select"))
                        });
                        if sql_is_select {
                            return format!("{module}::query_scalar::<_, String>({raw_args})");
                        }
                        return format!("{module}::query({raw_args})");
                    }
                    return format!("{module}::{fn_name}({raw_args})");
                }
            }
            // If the struct has an `id` field and the caller doesn't provide it
            // (arg count is one fewer than expected), auto-insert Uuid::new_v4() as first arg.
            let has_id_field = ctx.struct_fields.get(&effective_target)
                .map(|fields| fields.iter().any(|(n, _)| n == "id"))
                .unwrap_or(false);
            let final_args = if has_id_field && !call.args.is_empty() {
                // Check if first arg is already named 'id' — if so, caller is providing it
                let first_is_id = matches!(&call.args[0], Expr::Ident(n) if n == "id");
                if first_is_id {
                    cloned // caller provides id explicitly
                } else {
                    // Prepend auto-generated id
                    format!("Uuid::new_v4(), {}", cloned)
                }
            } else if has_id_field && call.args.is_empty() {
                "Uuid::new_v4()".to_string()
            } else {
                cloned
            };
            // If the constructor returns Result (invariant type), append ? to unwrap
            let returns_result = ctx.method_returns.get(&(effective_target.clone(), "new".to_string()))
                .map(|t| t.starts_with("Result<"))
                .unwrap_or(false);
            let suffix = if returns_result { "?" } else { "" };

            // Zero-arg smart ctors (`Default`): `T.new(a, b, c)` → positional
            // field fill + `..T::default()`. Skips a leading `id: Uuid` so
            // `Greeting.new(message)` still maps onto `message`, not `id`.
            if ctx.defaultable_types.contains(&effective_target) && !call.args.is_empty() {
                if let Some(fields) = ctx.struct_fields.get(&effective_target) {
                    let mut field_iter = fields.iter().peekable();
                    let mut parts: Vec<String> = Vec::new();
                    if let Some((fname, fty)) = field_iter.peek() {
                        if *fname == "id" && (*fty == "Uuid" || *fty == "uuid::Uuid") {
                            parts.push("id: Uuid::new_v4()".to_string());
                            field_iter.next();
                        }
                    }
                    for arg in &call.args {
                        if let Some((fname, _)) = field_iter.next() {
                            parts.push(format!(
                                "{}: {}",
                                to_snake(fname),
                                expr_to_rust(arg, ctx)
                            ));
                        }
                    }
                    parts.push(format!("..{}::default()", qualified));
                    return format!("{} {{ {} }}", qualified, parts.join(", "));
                }
            }

            return format!("{}::{}({}){}", qualified, to_snake(method), final_args, suffix);
        }
        // Language primitives (not product facades):
        // - Blob.from_hex / Blob.from_file for binary payloads
        // - Process.run(program, args, cwd) for host process execution
        let method_bare = method.trim_end_matches(['!', '?']);
        if (effective_target == "Blob" || effective_target.ends_with("Blob"))
            && method_bare == "from_hex"
            && call.args.len() == 1
        {
            let hex_expr = expr_to_rust(&call.args[0], ctx);
            return format!(
                "{}::new({})",
                stub_ctor_path(ctx, "Blob"),
                bytes_from_hex_expr(&hex_expr)
            );
        }
        if (effective_target == "Blob" || effective_target.ends_with("Blob"))
            && method_bare == "from_file"
            && call.args.len() == 1
        {
            let path_expr = expr_to_rust(&call.args[0], ctx);
            return format!(
                "{}::new(std::fs::read({path_expr}.as_str()).map_err(|e| DomainError::External(e.to_string()))?)",
                stub_ctor_path(ctx, "Blob")
            );
        }
        if effective_target == "Process" && method_bare == "run" && call.args.len() == 3 {
            let prog = expr_to_rust(&call.args[0], ctx);
            let args = expr_to_rust(&call.args[1], ctx);
            let cwd = expr_to_rust(&call.args[2], ctx);
            let hard = call.method.ends_with('!');
            // Soft Process.run returns detail String (incl. failed:); Process.run! aborts on non-zero.
            if hard {
                return format!(
                    "{{ let __prog: String = ({prog}).to_string(); let __args: String = ({args}).to_string(); let __cwd: String = ({cwd}).to_string(); let __argv: Vec<&str> = __args.split_whitespace().collect(); let __out = std::process::Command::new(&__prog).args(&__argv).current_dir(&__cwd).output().map_err(|e| DomainError::External(format!(\"{{e:?}}\")))?; if !__out.status.success() {{ let __err = String::from_utf8_lossy(&__out.stderr); let __tail: String = __err.chars().rev().take(2000).collect::<String>().chars().rev().collect(); return Err(DomainError::External(format!(\"{{}} failed: {{}}\", __prog, __tail))); }} format!(\"{{}} ok: {{}}\", __prog, String::from_utf8_lossy(&__out.stdout).chars().take(500).collect::<String>()) }}"
                );
            }
            return format!(
                "{{ let __prog: String = ({prog}).to_string(); let __args: String = ({args}).to_string(); let __cwd: String = ({cwd}).to_string(); let __argv: Vec<&str> = __args.split_whitespace().collect(); match std::process::Command::new(&__prog).args(&__argv).current_dir(&__cwd).output() {{ Ok(__out) => {{ if __out.status.success() {{ format!(\"{{}} ok: {{}}\", __prog, String::from_utf8_lossy(&__out.stdout).chars().take(400).collect::<String>()) }} else {{ let __err = String::from_utf8_lossy(&__out.stderr); let __tail: String = __err.chars().rev().take(1200).collect::<String>().chars().rev().collect(); format!(\"{{}} failed: {{}}\", __prog, __tail) }} }}, Err(e) => format!(\"{{}} spawn failed: {{e}}\", __prog) }} }}"
            );
        }
        // Non-new method on a struct: UFCS instance form `Email.validate(email)`
        // → `email.validate()`. Only when the first arg *names* the type
        // (Email/email). Do NOT rewrite for any local — that breaks enum
        // constructors: `AttributeValue.S(name)` must stay `AttributeValue::S(name)`.
        // PascalCase methods are always associated constructors / variants.
        let is_pascal_ctor = method
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false);
        if !is_pascal_ctor && !call.args.is_empty() {
            if let Expr::Ident(first_arg) = &call.args[0] {
                if first_arg.eq_ignore_ascii_case(&effective_target) {
                    let rest_args = call.args[1..]
                        .iter()
                        .map(|a| expr_to_rust(a, ctx))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return format!("{}.{}({})", first_arg, to_snake(method), rest_args);
                }
            }
        }
        // Enum variant constructor: AttributeValue.S(pk) → AttributeValue::S(pk)
        // No suffix needed — variant constructors are plain sync calls.
        // Stub enums are stored as Shape::Struct, so also check PascalCase method name
        // which indicates a variant constructor (e.g. AttributeValue.S(pk)).
        if ctx.name_to_shape.get(effective_target.as_str()) == Some(&Shape::Enum) || is_pascal_ctor {
            let m = rust_method_name(method);
            return format!("{}::{}({})", qualified, m, args_str);
        }
        // Prefer stub-qualified path (aws_sdk_s3::Client) over VEIL alias (S3Client).
        // Keep PascalCase for enum variants: AttributeValue::S(x), not ::s(x).
        // Use `cloned` so idents/field access (e.g. repo.slug) are not moved.
        let m = rust_method_name(method);
        let suffix = receiver_call_suffix(
            &Expr::Ident(effective_target.clone()),
            method,
            ctx,
        );
        return format!("{}::{}({}){}", qualified, m, cloned, suffix);
    }

    // `local.field.method(args)` — parser keeps dotted target "initiative.id".
    // Emit `initiative.id.method(...)`, never `id::method(...)`.
    if call.target.contains('.') && !call.target.starts_with("self.") {
        let first = call.target.split('.').next().unwrap_or("");
        if ctx.is_local(first) {
            let path = call
                .target
                .split('.')
                .enumerate()
                .map(|(i, seg)| {
                    if i == 0 {
                        seg.to_string()
                    } else {
                        to_snake(seg)
                    }
                })
                .collect::<Vec<_>>()
                .join(".");
            let method = rust_method_name(&call.method);
            let suffix = receiver_call_suffix(
                &Expr::Ident(first.to_string()),
                &call.method,
                ctx,
            );
            // Clone String fields when calling to_string-like methods is unnecessary;
            // for by-value SDK args, Uuid/DateTime Display paths use to_string().
            return format!(
                "{}.{}({}){}",
                path,
                method,
                clone_args_for_method(&call.method, &call.args, ctx),
                suffix
            );
        }
    }

    // Self field target (method bodies) → self.target.method(args)
    // Parser may produce target "client" or dotted "self.client".
    if ctx.in_method {
        let field = call
            .target
            .strip_prefix("self.")
            .unwrap_or(call.target.as_str());
        if ctx.self_fields.contains(field)
            || call.target.starts_with("self.")
        {
            let method = rust_method_name(&call.method);
            let suffix = receiver_call_suffix(
                &Expr::Ident(field.to_string()),
                &call.method,
                ctx,
            );
            // Map/HashMap fields wrapped in RwLock need lock acquisition and
            // reference-passing for key arguments (get/contains_key/remove take &Q).
            let field_type = ctx.self_field_types.get(field).or_else(|| ctx.self_field_types.get(&to_snake(field)));
            let is_map_field = field_type
                .map(|t| t.contains("HashMap") || t.starts_with("std::collections::HashMap"))
                .unwrap_or(false);
            if is_map_field {
                let bare_method = call.method.trim_end_matches(['!', '?']);
                match bare_method {
                    "get" | "contains_key" => {
                        // Read-only access: acquire read lock, pass key by reference.
                        // For `get`, append `.cloned()` so the returned value is owned
                        // and does not borrow the lock guard.
                        let key_arg = if !call.args.is_empty() {
                            let s = expr_to_rust(&call.args[0], ctx);
                            format!("&{}", s)
                        } else {
                            String::new()
                        };
                        let clone_suffix = if bare_method == "get" { ".cloned()" } else { "" };
                        return format!(
                            "self.{}.read().await.{}({}){}",
                            to_snake(field),
                            method,
                            key_arg,
                            clone_suffix,
                        );
                    }
                    "insert" => {
                        // Mutating access: acquire write lock
                        let map_args = call.args.iter()
                            .map(|a| {
                                let s = expr_to_rust(a, ctx);
                                match a {
                                    Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("{}.clone()", s),
                                    _ => s,
                                }
                            }).collect::<Vec<_>>().join(", ");
                        return format!(
                            "self.{}.write().await.insert({})",
                            to_snake(field),
                            map_args,
                        );
                    }
                    "remove" => {
                        // Mutating access: acquire write lock, pass key by reference
                        let key_arg = if !call.args.is_empty() {
                            let s = expr_to_rust(&call.args[0], ctx);
                            format!("&{}", s)
                        } else {
                            String::new()
                        };
                        return format!(
                            "self.{}.write().await.remove({})",
                            to_snake(field),
                            key_arg,
                        );
                    }
                    "values" | "keys" | "iter" | "len" | "is_empty" => {
                        // Read-only access, no key arg
                        return format!(
                            "self.{}.read().await.{}({})",
                            to_snake(field),
                            method,
                            clone_args_for_method(&call.method, &call.args, ctx),
                        );
                    }
                    _ => {
                        // Other methods: default to write lock (safe fallback)
                        return format!(
                            "self.{}.write().await.{}({})",
                            to_snake(field),
                            method,
                            clone_args_for_method(&call.method, &call.args, ctx),
                        );
                    }
                }
            }
            return format!(
                "self.{}.{}({}){}",
                to_snake(field),
                method,
                clone_args_for_method(&call.method, &call.args, ctx),
                suffix
            );
        }
    }

    // Local variable target → target.method(args)?
    if ctx.is_local(&call.target) {
        // Always strip VEIL `!`/`?` fallible/query suffixes (typecheck sugar only).
        let method = rust_method_name(&call.method);

        // Blob / Bytes / unknown locals: `.to_str()` is utf-8 decode, not a
        // rustc method on the stub type. Leave `.to_string()` alone.
        if call.args.is_empty()
            && matches!(
                call.method.trim_end_matches(['!', '?']),
                "to_str" | "as_str"
            )
        {
            let ty = ctx.local_type(&call.target).unwrap_or("");
            if ty != "String" {
                return format!(
                    "String::from_utf8_lossy({}.as_ref()).to_string()",
                    call.target
                );
            }
        }
        if call.args.is_empty() && method_bare(&call.method) == "as_ref" {
            let recv_ident = Expr::Ident(call.target.clone());
            if should_decode_as_ref_to_str(&recv_ident, ctx) {
                return format!(
                    "String::from_utf8_lossy({}.as_ref()).to_string()",
                    call.target
                );
            }
        }

        // HashMap/DynamoDB item .get("key") — never panic (review Issue 6).
        if call.method == "get" && call.args.len() == 1 {
            if let Expr::StringLit(key) = &call.args[0] {
                return format!(
                    "{}.get(\"{}\").ok_or_else(|| DomainError::External(\"missing {}\".into()))?",
                    call.target, key, key
                );
            }
        }
        // Option.unwrap() → ok_or; Result.unwrap() → map_err to DomainError.
        // Clone Option first so the local can be reused after is_some()/unwrap.
        if (call.method == "unwrap" || call.method == "unwrap!") && call.args.is_empty() {
            let ty = ctx.local_type(&call.target);
            if ty.map(|t| t.starts_with("Result<")).unwrap_or(false) {
                return format!(
                    "{}.map_err(|e| DomainError::External(format!(\"{{e}}\")))?",
                    call.target
                );
            }
            let is_option = ty
                .map(|t| t.starts_with("Option<"))
                .unwrap_or(true); // default to true if type unknown
            if is_option {
                // When the enclosing function returns Option<T>, use `?` directly
                // on the Option (returns None early) instead of converting to Result.
                let enclosing_returns_option = ctx.expected_return_rust.as_ref()
                    .map(|r| r.starts_with("Option<"))
                    .unwrap_or(false);
                if enclosing_returns_option {
                    return format!("{}.clone()?", call.target);
                }
                return format!(
                    "{}.clone().ok_or(DomainError::NotFound)?",
                    call.target
                );
            } else {
                // Already unwrapped — just use the value
                return call.target.clone();
            }
        }
        // local.ok_or(...) when local is NOT Option → skip, just use the local.
        if call.method == "ok_or" && ctx.is_local(&call.target) {
            let is_option = ctx.local_type(&call.target)
                .map(|t| t.starts_with("Option<"))
                .unwrap_or(true);
            if !is_option {
                return call.target.clone();
            }
        }
        if let Some(type_name) = ctx.local_type(&call.target) {
            // JSON value locals: translate common methods to serde_json equivalents.
            if type_name == "serde_json::Value" {
                match call.method.as_str() {
                    "len" => return format!("{}.as_array().map(|a| a.len() as i64).unwrap_or(0)", call.target),
                    "is_empty" => return format!("{}.as_array().map(|a| a.is_empty()).unwrap_or(true)", call.target),
                    "to_string" | "to_str" => return format!("{}.as_str().unwrap_or(\"\").to_string()", call.target),
                    _ => {}
                }
            }
            // If the local's type is a known trait, methods are async. Only
            // apply `?` when the port method is fallible (Res! / bang).
            if ctx.name_to_shape.get(type_name) == Some(&Shape::Trait) {
                let bare_ty = peel_dyn_trait_name(type_name).unwrap_or_else(|| type_name.to_string());
                let fallible = call.method.ends_with('!')
                    || ctx
                        .type_fallible_methods
                        .contains(&(bare_ty, method.clone()))
                    || ctx
                        .type_fallible_methods
                        .contains(&(type_name.to_string(), method.clone()));
                let suffix = if fallible { ".await?" } else { ".await" };
                return format!("{}.{}({}){}", call.target, method, args_str, suffix);
            }
            // Auto-unwrap Option<T> locals when calling a method that belongs to T.
            // This handles the common pattern: `provider = repo.find!(id)` then
            // `provider.get_endpoint(...)` where provider is Option<ApiProvider>.
            if type_name.starts_with("Option<") {
                let bare_method = call.method.trim_end_matches(['!', '?']);
                let option_methods = [
                    "is_some", "is_none", "unwrap", "unwrap_or", "unwrap_or_else",
                    "unwrap_or_default", "map", "and_then", "or_else", "ok_or",
                    "ok_or_else", "as_ref", "as_mut", "take", "replace", "clone",
                    "expect", "filter", "flatten", "zip",
                ];
                if !option_methods.contains(&bare_method) {
                    let cloned_args = clone_args_for_method(&call.method, &call.args, ctx);
                    return format!(
                        "{}.clone().ok_or(DomainError::NotFound)?.{}({})",
                        call.target, method, cloned_args
                    );
                }
                // Consuming Option methods (and_then, map, unwrap, filter, etc.)
                // move self — clone to allow reuse of the local variable.
                let non_consuming = ["is_some", "is_none", "as_ref", "as_mut", "clone"];
                if !non_consuming.contains(&bare_method) {
                    let cloned_args = clone_args_for_method(&call.method, &call.args, ctx);
                    let suffix = receiver_call_suffix(
                        &Expr::Ident(call.target.clone()),
                        &call.method,
                        ctx,
                    );
                    // unwrap_or with a string lit on Option<String> needs .to_string()
                    if (bare_method == "unwrap_or" || bare_method == "unwrap_or_else")
                        && call.args.len() == 1
                    {
                        if let Expr::StringLit(s) = &call.args[0] {
                            let lit = s.replace('\\', "\\\\").replace('"', "\\\"");
                            return format!(
                                "{}.clone().{}(\"{}\".to_string()){}",
                                call.target, method, lit, suffix
                            );
                        }
                    }
                    return format!(
                        "{}.clone().{}({}){}",
                        call.target, method, cloned_args, suffix
                    );
                }
            }
            // Known concrete method (e.g. aggregate fn) — call with ?
            if ctx.method_returns.contains_key(&(type_name.to_string(), call.method.clone()))
                || ctx.method_returns.contains_key(&(
                    type_name.to_string(),
                    call.method.trim_end_matches(['!', '?']).to_string(),
                ))
            {
                let cloned_args = clone_args_for_typed_method(Some(&type_name), &call.method, &call.args, ctx);
                let suffix = receiver_call_suffix(
                    &Expr::Ident(call.target.clone()),
                    &call.method,
                    ctx,
                );
                return format!("{}.{}({}){}", call.target, method, cloned_args, suffix);
            }
        }
        // Stub getters that return Result<&str, _> (e.g. enum as_s): own a String.
        // Method may be written `as_s!` — lookup is on the bare name.
        if call.args.is_empty() {
            let recv_ty = ctx.local_type(&call.target).map(|s| s.to_string());
            if should_own_str_result(ctx, recv_ty.as_deref(), &call.method) {
                let m = method_bare(&call.method);
                return format!("{}.{m}(){}", call.target, map_err_domain_own_str());
            }
        }
        // serde_json::Value::as_str → Option<String> so assigns/unwrap are owned.
        if call.method == "as_str" && call.args.is_empty() {
            return format!("{}.as_str().map(|s| s.to_string())", call.target);
        }
        // Unknown method on local — clone args to avoid move issues.
        // Collection predicate methods need .iter() prefix in Rust.
        let iter_methods = ["any", "all", "find", "filter", "map", "for_each", "count", "flat_map"];
        if iter_methods.contains(&method.as_str()) {
            return format!(
                "{}.iter().{}({})",
                call.target,
                method,
                clone_args_for_method(&call.method, &call.args, ctx)
            );
        }
        let suffix = receiver_call_suffix(
            &Expr::Ident(call.target.clone()),
            &call.method,
            ctx,
        );
        // unwrap_or on Option<String> needs owned default; Option<&str> (e.g. after
        // `.as_str()`) needs a bare str. Prefer owned — callers of as_str use the
        // chained-receiver path below.
        let bare_m = call.method.trim_end_matches(['!', '?']);
        if (bare_m == "unwrap_or" || bare_m == "unwrap_or_else") && call.args.len() == 1 {
            if let Expr::StringLit(s) = &call.args[0] {
                return format!(
                    "{}.{}(\"{}\".to_string()){}",
                    call.target, method, s, suffix
                );
            }
        }
        // Auto-unwrap Option<T> args passed to container methods like push/insert.
        // When an Option<T> local is pushed into a Vec<T>, unwrap it first.
        if (bare_m == "push" || bare_m == "insert" || bare_m == "extend") && !call.args.is_empty() {
            if let Some(Expr::Ident(arg_name)) = call.args.first() {
                if let Some(ty) = ctx.local_type(arg_name) {
                    if ty.starts_with("Option<") {
                        let rest_args = if call.args.len() > 1 {
                            format!(", {}", clone_args_for_method(&call.method, &call.args[1..], ctx))
                        } else {
                            String::new()
                        };
                        return format!(
                            "{}.{}({}.clone().ok_or(DomainError::NotFound)?{}){}",
                            call.target, method, arg_name, rest_args, suffix
                        );
                    }
                }
            }
        }
        // Resolve target type for ref-param passing
        let target_type: Option<&str> = ctx.local_type(&call.target);
        return format!(
            "{}.{}({}){}",
            call.target,
            method,
            clone_args_for_typed_method(target_type, &call.method, &call.args, ctx),
            suffix
        );
    }
    if call.method.is_empty() {
        // Bare call: now() → Utc::now(), others → as-is (cloning value args so
        // passing locals/state into a by-value param doesn't move them).
        // Bang form `name!(args)` stores target as `name!` — strip for symbol, keep `?`.
        let bare_target = call.target.trim_end_matches(['!', '?']);
        match bare_target {
            "now" => "Utc::now()".to_string(),
            "drop" => {
                // Rust builtin drop() — pass through without cloning.
                let args_str = call.args.iter()
                    .map(|a| expr_to_rust(a, ctx))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("drop({})", args_str)
            }
            _ => {
                // Bare dep-method resolution: `authenticate!()` → `deps.auth.authenticate().await?`
                // when `authenticate` matches a method on an in-scope dep. Two strategies:
                // 1. Exact method match in method_returns (formally declared dep methods)
                // 2. Dep field name prefix match (e.g. dep "auth" → call "authenticate")
                let dep_method_match = ctx.dep_fields.iter().find_map(|(trait_name, field_name)| {
                    // Strategy 1: bare_target is a registered method on this trait
                    let key = (trait_name.clone(), bare_target.to_string());
                    if ctx.method_returns.contains_key(&key) {
                        return Some(field_name.clone());
                    }
                    let key2 = (field_name.clone(), bare_target.to_string());
                    if ctx.method_returns.contains_key(&key2) {
                        return Some(field_name.clone());
                    }
                    // Strategy 2: bare_target starts with the dep field name
                    // (e.g. dep "auth" → call "authenticate", dep "check_scope" → call "check_scope")
                    if bare_target.starts_with(field_name.as_str())
                        && (bare_target.len() == field_name.len()
                            || bare_target.as_bytes().get(field_name.len()) == Some(&b'_')
                            || bare_target[field_name.len()..].chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false))
                    {
                        return Some(field_name.clone());
                    }
                    None
                });
                if let Some(dep_field) = dep_method_match {
                    let args_str = clone_args(&call.args, ctx);
                    return format!(
                        "deps.{}.{}({}).await?",
                        dep_field,
                        to_snake(bare_target),
                        args_str
                    );
                }
                let base = format!(
                    "{}({})",
                    to_snake(bare_target),
                    clone_args(&call.args, ctx)
                );
                let is_bang = call.target.ends_with('!');
                // Layer-declared async functions (e.g. unwind, run_saga) need .await?
                if ctx.async_fns.contains(bare_target) || ctx.async_fns.contains(&call.target)
                {
                    format!("{}.await?", base)
                } else if is_bang {
                    format!("{}?", base)
                } else {
                    base
                }
            }
        }
    } else if ctx.is_local(&call.target) || ctx.name_to_shape.contains_key(&call.target) {
        // Known local/construct method call (already handled above, but be safe).
        format!("{}.{}({})", call.target, to_snake(&call.method), args_str)
    } else {
        // Unknown target with a method (e.g. `http.post(...)`): an external
        // effect. Route it to a generated runtime hook `<target>_<method>(...)`
        // so the code compiles without inventing domain knowledge. The set of
        // hooks is emitted at the bottom of the module.
        //
        // If target has dots (e.g. `sqlx.Query`), the last segment is the
        // struct name — emit `Struct::method(args)` (Rust path syntax).
        // Skip `self.field` — already handled above when in_method.
        if call.target.contains('.') && !call.target.starts_with("self.") {
            let parts: Vec<&str> = call.target.split('.').collect();
            let struct_name = parts.last().unwrap_or(&"");
            // Qualify via stub map when present
            let qualified = if let Some((crate_name, original_name)) =
                ctx.stub_type_crate.get(*struct_name).or_else(|| {
                    // case-insensitive match for Client vs client
                    ctx.stub_type_crate
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
            let suffix = if bare == "send"
                || bare == "send_with"
                || ctx.async_fallible_methods.contains(bare)
            {
                ".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?"
            } else if ctx.fallible_methods.contains(bare) {
                "?"
            } else {
                ""
            };
            return format!("{}::{}({}){}", qualified, m, args_str, suffix);
        }
        // Recognize Rust module-qualified calls: serde_json.from_str, std.fs.read, etc.
        // These are lowercase targets with no dots that map to Rust crate paths using `::`.
        let known_modules = [
            "serde_json", "serde", "tokio", "tracing", "uuid", "chrono",
            "std", "aws_sdk_dynamodb", "aws_sdk_s3", "aws_config",
        ];
        let target_snake = to_snake(&call.target);
        if known_modules.contains(&target_snake.as_str()) {
            let m = to_snake(&call.method);
            let suffix = if ctx.fallible_methods.contains(&call.method)
                || call.method == "from_str"
                || call.method == "to_string"
                || call.method == "parse"
            {
                "?"
            } else {
                ""
            };
            // serde_json.from_str → serde_json::from_str(&arg)?
            // serde_json.to_string → serde_json::to_string(&arg)?
            let needs_ref = m == "from_str" || m == "to_string" || m == "to_vec";
            let final_args = if needs_ref && call.args.len() == 1 {
                format!("&{}", expr_to_rust(&call.args[0], ctx))
            } else {
                args_str.clone()
            };
            // from_str needs a turbofish when the enclosing method return type
            // names a concrete domain type (else inference fails with `?`).
            if target_snake == "serde_json" && m == "from_str" {
                if let Some(ty) = from_str_turbofish_type(ctx) {
                    return format!(
                        "serde_json::from_str::<{ty}>({final_args}){suffix}"
                    );
                }
            }
            return format!("{}::{}({}){}", target_snake, m, final_args, suffix);
        }
        // Stub package free functions: `crypto.hmac_sha256_hex(s, m)` or
        // `relay_crypto.aes_gcm_encrypt!(k, p)` → `relay_crypto::fn(&…)` (+ `?` if Res!).
        if let Some(rust_crate) = ctx
            .stub_pkg_crate
            .get(&call.target)
            .or_else(|| ctx.stub_pkg_crate.get(&target_snake))
        {
            let bare = call.method.trim_end_matches(['!', '?']);
            if let Some(&fallible) = ctx
                .stub_free_fns
                .get(&(rust_crate.clone(), bare.to_string()))
            {
                let m = to_snake(bare);
                // Helper crates typically take &str / shared refs.
                let final_args = call
                    .args
                    .iter()
                    .map(|a| {
                        let s = expr_to_rust(a, ctx);
                        match a {
                            Expr::StringLit(_) => format!("&{s}"),
                            Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("&{s}"),
                            _ => {
                                if s.starts_with('&') {
                                    s
                                } else {
                                    format!("&({s})")
                                }
                            }
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let suffix = if fallible {
                    ".map_err(|e| DomainError::External(e.to_string()))?"
                } else {
                    ""
                };
                return format!("{rust_crate}::{m}({final_args}){suffix}");
            }
        }
        // Last resort: target is not a known local, construct, self-field,
        // module, or stub. It is either (a) an external-effect target (e.g.
        // `http.post(...)`) that should be flattened to `http_post(args)` to
        // match the generated runtime-hook stubs, or (b) a closure/iterator
        // parameter calling a method. Closure params are now properly tracked
        // in ctx.locals (see Closure branch above), so anything reaching here
        // IS an external effect — emit the flattened hook form.
        let m_clean = call.method.trim_end_matches(['!', '?']);
        let target_is_var_like = call.target.chars().next()
            .map(|c| c.is_lowercase())
            .unwrap_or(false)
            && !call.target.contains('.');
        if target_is_var_like {
            // Phase 2, Issue 2: .get("key") on closure params — emit bare &str,
            // not .to_string(). Also emit .ok_or_else(...)? to unwrap the Option.
            if m_clean == "get" && call.args.len() == 1 {
                if let Expr::StringLit(key) = &call.args[0] {
                    return format!(
                        "{}.get(\"{}\").ok_or_else(|| DomainError::External(\"missing {}\".into()))?",
                        call.target, key, key
                    );
                }
            }
            // Phase 2, Issue 1: .unwrap() on closure params that are already extracted
            if (m_clean == "unwrap" || m_clean == "unwrap!") && call.args.is_empty() {
                // In closure contexts the value is typically already unwrapped — just return target
                // This handles cases where the closure param was already unwrapped by the chain
                return call.target.clone();
            }
            // Phase 2: as_s / as_n on closure params (DDB AttributeValue)
            if call.args.is_empty()
                && should_own_str_result(ctx, ctx.local_type(&call.target), &call.method)
            {
                return format!(
                    "{}.{}(){}",
                    call.target,
                    m_clean,
                    map_err_domain_own_str()
                );
            }
        }
        // Not on a stub type / construct / local. Do not invent a crate or a
        // no-op hook — the .stub is the only third-party contract.
        format!(
            "{{ compile_error!(\"unstubbed external `{}.{}` — install a .stub and call its types (@field + stub methods)\"); }}",
            call.target,
            m_clean
        )
    }
}

/// Classify a `guard` failure message → DomainError variant.
/// Missing/forbidden resources use NotFound (404, no enumeration via 400).
/// Real input validation stays Validation (400).
fn guard_error_variant(msg: &str) -> &'static str {
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
fn translate_action(a: &ActionExpr, ctx: &GenCtx) -> String {
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
                        format!("{base}.map_err(|_| DomainError::NotFound)?")
                    } else {
                        format!(
                            "{base}.map_err(|_| DomainError::Validation(\"{msg_escaped}\".to_string()))?"
                        )
                    }
                }
                Some(cond @ Expr::Await(_)) => {
                    let call_str = expr_to_rust(cond, ctx);
                    let base = call_str.strip_suffix('?').unwrap_or(&call_str);
                    if err_var == "NotFound" {
                        format!("{base}.map_err(|_| DomainError::NotFound)?")
                    } else {
                        format!(
                            "{base}.map_err(|_| DomainError::Validation(\"{msg_escaped}\".to_string()))?"
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
                                "DomainError::NotFound".to_string()
                            } else {
                                format!("DomainError::Validation(\"{msg_escaped}\".to_string())")
                            };
                            return format!(
                                "if {}.is_none() {{ return Err({err}); }}",
                                c.target
                            );
                        }
                    }
                    let err = if err_var == "NotFound" {
                        "DomainError::NotFound".to_string()
                    } else {
                        format!("DomainError::Validation(\"{msg_escaped}\".to_string())")
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
        _ => format!("    {};", expr_to_rust(expr, ctx)),
    }
}

/// Binding names introduced by a match arm pattern string (e.g. `Some(item)` → `item`).
fn pattern_binding_names(pattern: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut cur = String::new();
    let mut in_ident = false;
    for ch in pattern.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cur.push(ch);
            in_ident = true;
        } else if in_ident {
            // Skip keywords / constructors (Some, None, Ok, Err, true, false)
            let skip = matches!(
                cur.as_str(),
                "Some" | "None" | "Ok" | "Err" | "true" | "false" | "_"
            ) || cur
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false);
            if !skip && !cur.is_empty() {
                names.push(cur.clone());
            }
            cur.clear();
            in_ident = false;
        }
    }
    if in_ident {
        let skip = matches!(
            cur.as_str(),
            "Some" | "None" | "Ok" | "Err" | "true" | "false" | "_"
        ) || cur
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false);
        if !skip && !cur.is_empty() {
            names.push(cur);
        }
    }
    names
}

/// Convert a structured Pattern to Rust pattern syntax.
pub fn pattern_to_rust(pat: &Pattern) -> String {
    pattern_to_rust_qualified(pat, None)
}

fn qualify_variant_name(name: &str, enums: Option<&HashMap<String, String>>) -> String {
    if name.contains("::") {
        return name.to_string();
    }
    if name.contains('.') && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        return name.replace('.', "::");
    }
    if let Some(en) = enums.and_then(|m| m.get(name)) {
        return format!("{en}::{name}");
    }
    name.to_string()
}

fn pattern_to_rust_qualified(
    pat: &Pattern,
    enums: Option<&HashMap<String, String>>,
) -> String {
    match pat {
        Pattern::Ident(s) => {
            if s.contains('.') && s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                s.replace('.', "::")
            } else if let Some(en) = enums.and_then(|m| m.get(s)) {
                format!("{en}::{s}")
            } else if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                s.clone()
            } else {
                to_snake(s)
            }
        }
        Pattern::Tuple(parts) => {
            let inner = parts
                .iter()
                .map(|p| pattern_to_rust_qualified(p, enums))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({})", inner)
        }
        Pattern::Struct(name, fields, has_rest) => {
            let rust_name = qualify_variant_name(name, enums);
            let mut fs: Vec<String> = fields
                .iter()
                .map(|(k, v)| match v {
                    Some(pat) => format!(
                        "{}: {}",
                        to_snake(k),
                        pattern_to_rust_qualified(pat, enums)
                    ),
                    None => to_snake(k),
                })
                .collect();
            if *has_rest {
                fs.push("..".to_string());
            }
            format!("{} {{ {} }}", rust_name, fs.join(", "))
        }
        Pattern::Variant(name, args) => {
            let rust_name = qualify_variant_name(name, enums);
            if args.is_empty() {
                rust_name
            } else {
                let inner = args
                    .iter()
                    .map(|p| pattern_to_rust_qualified(p, enums))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", rust_name, inner)
            }
        }
        Pattern::Literal(s) => s.clone(),
        Pattern::Or(alts) => alts
            .iter()
            .map(|p| pattern_to_rust_qualified(p, enums))
            .collect::<Vec<_>>()
            .join(" | "),
        Pattern::Wildcard => "_".to_string(),
        Pattern::Rest => "..".to_string(),
    }
}

impl GenCtx {
    /// A shallow clone carrying just the maps needed for type inference (used
    /// by the return-type pre-scan in rust.rs).
    pub fn clone_for_inference(&self) -> GenCtx {
        GenCtx {
            name_to_shape: self.name_to_shape.clone(),
            locals: self.locals.clone(),
            self_fields: self.self_fields.clone(),
            in_method: self.in_method,
            envelope_routing: self.envelope_routing,
            method_returns: self.method_returns.clone(),
            method_params: self.method_params.clone(),
            ref_params: self.ref_params.clone(),
            local_types: self.local_types.clone(),
            struct_fields: self.struct_fields.clone(),
            routing_ref: self.routing_ref.clone(),
            routing_traits: self.routing_traits.clone(),
            async_fns: self.async_fns.clone(),
            state_locals: self.state_locals.clone(),
            stub_type_crate: self.stub_type_crate.clone(),
            stub_typed_ctors: self.stub_typed_ctors.clone(),
            fallible_methods: self.fallible_methods.clone(),
            non_fallible_methods: self.non_fallible_methods.clone(),
            type_fallible_methods: self.type_fallible_methods.clone(),
            async_fallible_methods: self.async_fallible_methods.clone(),
            expected_return_rust: self.expected_return_rust.clone(),
            option_value_wrap: self.option_value_wrap,
            defaultable_types: self.defaultable_types.clone(),
            dep_fields: self.dep_fields.clone(),
            mut_locals: self.mut_locals.clone(),
            stub_pkg_crate: self.stub_pkg_crate.clone(),
            stub_free_fns: self.stub_free_fns.clone(),
            bus_returns: self.bus_returns.clone(),
            local_domain_types: self.local_domain_types.clone(),
            self_field_types: self.self_field_types.clone(),
            statement_specs: self.statement_specs.clone(),
            enum_variants: self.enum_variants.clone(),
        }
    }
}

/// Infer turbofish type for `serde_json::from_str` from the enclosing return type.
fn from_str_turbofish_type(ctx: &GenCtx) -> Option<String> {
    let ret = ctx.expected_return_rust.as_deref()?;
    // Result<Option<T>, _> / Result<Vec<T>, _> / Result<T, _> / Option<T>
    let mut s = ret.trim();
    if let Some(inner) = s.strip_prefix("Result<").and_then(|x| {
        // split last , DomainError>
        let depth = 0i32;
        let _ = depth;
        x.rsplit_once(", ").map(|(a, _)| a.trim())
    }) {
        s = inner;
    }
    if let Some(inner) = s.strip_prefix("Option<").and_then(|x| x.strip_suffix('>')) {
        s = inner.trim();
    }
    if let Some(inner) = s.strip_prefix("Vec<").and_then(|x| x.strip_suffix('>')) {
        s = inner.trim();
    }
    // Domain types are PascalCase; skip Value / primitives.
    if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        && s != "DomainError"
        && !s.starts_with("Result")
    {
        return Some(s.to_string());
    }
    // OAuth token JSON and similar → Value
    Some("serde_json::Value".into())
}

/// Emit a block of statements, tracking locals so later lines see earlier binds
/// (needed for if/while/for bodies: `mut req = …` then `req = req.header(…)`).
fn emit_tracked_block(body: &[Expr], ctx: &GenCtx, indent: &str) -> String {
    emit_block_lines(body, ctx, indent, false)
}

/// Block used as a value: last expression has no semicolon (and may
/// `Some`-wrap when `ctx.option_value_wrap`).
fn emit_value_block(body: &[Expr], ctx: &GenCtx, indent: &str) -> String {
    emit_block_lines(body, ctx, indent, true)
}

fn emit_block_lines(body: &[Expr], ctx: &GenCtx, indent: &str, last_is_value: bool) -> String {
    let mut body_ctx = ctx.clone_for_inference();
    body_ctx.option_value_wrap = false;
    body_ctx.mut_locals.extend(analyze_mut_locals(body));
    let mut lines = Vec::new();
    for (i, e) in body.iter().enumerate() {
        let is_last = i + 1 == body.len();
        if is_last && last_is_value {
            body_ctx.option_value_wrap = ctx.option_value_wrap;
        }
        let line = expr_to_rust(e, &body_ctx);
        if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = e {
            if !name.contains('.') {
                body_ctx.locals.insert(name.clone());
                if let Some(t) = infer_expr_type(rhs, &body_ctx) {
                    body_ctx.local_types.insert(name.clone(), t);
                }
            }
        }
        if is_last && last_is_value {
            lines.push(format!("{indent}{line}"));
        } else {
            lines.push(format!("{indent}{line};"));
        }
    }
    lines.join("\n")
}

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
                    for e in &arm.body {
                        walk_mut_needs(e, &mut needs, &mut bound);
                    }
                }
            }
        }
    }
    needs
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

fn walk_mut_needs(expr: &Expr, needs: &mut HashSet<String>, bound: &mut HashSet<String>) {
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
                } else if let Some(recv) = &call.receiver {
                    if let Expr::Ident(n) = recv.as_ref() {
                        needs.insert(n.clone());
                    }
                }
            }
        }
        Expr::IfExpr(ie) => {
            walk_mut_needs(&ie.condition, needs, bound);
            for e in &ie.then_body {
                walk_mut_needs(e, needs, bound);
            }
            if let Some(eb) = &ie.else_body {
                for e in eb {
                    walk_mut_needs(e, needs, bound);
                }
            }
        }
        Expr::IfLet {
            expr: scrut,
            then_body,
            else_body,
            ..
        } => {
            walk_mut_needs(scrut, needs, bound);
            for e in then_body {
                walk_mut_needs(e, needs, bound);
            }
            if let Some(eb) = else_body {
                for e in eb {
                    walk_mut_needs(e, needs, bound);
                }
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
                if let Some(g) = &arm.guard {
                    walk_mut_needs(g, needs, bound);
                }
                for e in &arm.body {
                    walk_mut_needs(e, needs, bound);
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

/// Public wrapper for `infer_expr_type`, for the return-type pre-scan.
pub fn infer_expr_type_pub(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    infer_expr_type(expr, ctx)
}

/// Infer the element type of an iterable expression. If it's a local whose
/// tracked type is `Vec<T>` (or a boxed-trait vec), return the inner `T`
/// (unwrapping `Box<dyn T ..>` to `T`) so method calls on the loop var resolve.
fn element_type_of(iterable: &Expr, ctx: &GenCtx) -> Option<String> {
    let vec_type = match iterable {
        Expr::Ident(name) => {
            // Self fields in method bodies: `for x in api_endpoints` after bare-field rewrite.
            if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                // Look up via any struct_fields entry that has this field.
                ctx.struct_fields.values().find_map(|fields| {
                    fields
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, t)| t.clone())
                })
            } else {
                ctx.local_type(name).map(|s| s.to_string())
            }
        }
        Expr::FieldAccess(base, field) => {
            if let Expr::Ident(base_name) = base.as_ref() {
                if base_name == "self" && ctx.in_method {
                    ctx.struct_fields.values().find_map(|fields| {
                        fields
                            .iter()
                            .find(|(n, _)| n == field)
                            .map(|(_, t)| t.clone())
                    })
                } else if let Some(type_name) = ctx.local_type(base_name) {
                    ctx.field_type(type_name, field).map(|s| s.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }?;
    let inner = vec_type
        .strip_prefix("Vec<")
        .and_then(|s| s.strip_suffix('>'))
        .or_else(|| {
            // Also accept `std::collections::…` forms — take after last `Vec<`.
            vec_type
                .rfind("Vec<")
                .map(|i| &vec_type[i + 4..vec_type.len().saturating_sub(1)])
        })?;
    let inner = inner.trim();
    // Unwrap Box<dyn Trait + Send + Sync> → Trait.
    if let Some(rest) = inner.strip_prefix("Box<dyn ") {
        let name = rest.split([' ', '+', '>']).next().unwrap_or(rest);
        return Some(name.to_string());
    }
    Some(inner.to_string())
}

/// Infer the Rust type of a flow's return expression (`ret <expr>`).
/// Resolves idents and field access against known local/struct-field types.
pub fn infer_return_expr_type(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    match expr {
        Expr::IntLit(_) => Some("i64".to_string()),
        Expr::FloatLit(_) => Some("f64".to_string()),
        Expr::BoolLit(_) => Some("bool".to_string()),
        Expr::StringLit(_) | Expr::StringInterp(_) => Some("String".to_string()),
        Expr::Ident(name) => ctx.local_type(name).map(|s| s.to_string()),
        Expr::FieldAccess(base, field) => {
            // Resolve the base's type, then the field's declared type.
            if let Expr::Ident(name) = base.as_ref() {
                if let Some(type_name) = ctx.local_type(name) {
                    if type_name == "serde_json::Value" {
                        // Orchestrator: JSON index — type is Value.
                        return Some("serde_json::Value".to_string());
                    }
                    if let Some(ft) = ctx.field_type(type_name, field) {
                        return Some(rust_type_for_named(ft));
                    }
                }
            }
            None
        }
        Expr::Call(_) => infer_expr_type(expr, ctx),
        _ => None,
    }
}

/// Normalize a VEIL match pattern into Rust form. VEIL writes `Ok _` / `Err e`
/// (space-separated binding); Rust needs `Ok(_)` / `Err(e)`. A bare word or
/// already-parenthesized pattern is left as-is.
fn normalize_match_pattern(pattern: &str, ctx: &GenCtx) -> String {
    let p = pattern.trim();
    // Convert dot-separated variant paths to Rust :: syntax
    // e.g. "DeployUnitType.LambdaApi" → "DeployUnitType::LambdaApi"
    if p.contains('.') && p.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        let converted = p.replace('.', "::");
        // Check for variant-with-binding after conversion
        if let Some((head, rest)) = converted.split_once(char::is_whitespace) {
            let rest = rest.trim();
            if !rest.is_empty() && !rest.starts_with('(') && head.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                return format!("{}({})", head, rest);
            }
        }
        return converted;
    }
    // Enum-variant-with-binding: `Variant binding` → `Variant(binding)`.
    if let Some((head, rest)) = p.split_once(char::is_whitespace) {
        let rest = rest.trim();
        if !rest.is_empty() && !rest.starts_with('(') {
            // Only treat capitalized heads as variants (Ok, Err, Some, custom).
            if head.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                let head = qualify_variant_name(head, Some(&ctx.enum_variants));
                return format!("{}({})", head, rest);
            }
        }
    }
    qualify_variant_name(p, Some(&ctx.enum_variants))
}

/// Map a VEIL simple type name (as stored in struct_fields) to its Rust form.
fn rust_type_for_named(name: &str) -> String {
    match name {
        "Str" => "String".to_string(),
        "Int" => "i64".to_string(),
        "F64" => "f64".to_string(),
        "Bool" => "bool".to_string(),
        "Bytes" => "Vec<u8>".to_string(),
        "UUID" | "Id" => "Uuid".to_string(),
        "DateTime" | "Dt" => "DateTime<Utc>".to_string(),
        "Json" => "serde_json::Value".to_string(),
        other => other.to_string(),
    }
}

/// Default expression for a field type when a positional `Type.new(a, b)` call
/// omits trailing fields. Types are stored as Rust forms from struct_fields.
fn field_type_default_expr(rust_ty: &str, field_name: &str) -> String {
    let t = rust_ty.trim();
    if t.starts_with("Option<") {
        return "None".to_string();
    }
    if t.starts_with("Vec<") {
        return "Vec::new()".to_string();
    }
    if t.contains("HashMap") {
        return "std::collections::HashMap::new()".to_string();
    }
    if t.contains("HashSet") {
        return "std::collections::HashSet::new()".to_string();
    }
    match t {
        "String" => {
            // Conventional auth header name used by CreateProvider defaults.
            if field_name == "authorization_header_string" {
                "\"Authorization\".to_string()".to_string()
            } else {
                "String::new()".to_string()
            }
        }
        "i64" | "i32" | "u64" | "u32" | "usize" | "isize" => "0".to_string(),
        "f64" | "f32" => "0.0".to_string(),
        "bool" => "false".to_string(),
        "Uuid" => "Uuid::new_v4()".to_string(),
        "DateTime<Utc>" => "Utc::now()".to_string(),
        "serde_json::Value" => "serde_json::json!({})".to_string(),
        // Nested domain types / enums: prefer Default (emitted for all-defaultable
        // VOs; enums can derive or use first-variant Default later).
        other => format!("{}::default()", other),
    }
}

/// Attempt to infer the type of an expression from context.
fn infer_expr_type(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    match expr {
        Expr::Call(call) => {
            if call.receiver.is_none() && call.args.is_empty() {
                let leaf = lang_type_leaf(&call.target);
                let method = method_bare(&call.method);
                if matches!(
                    (leaf, method),
                    ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601")
                ) {
                    return Some("String".to_string());
                }
            }
            // Envelope routing: cross-boundary calls yield `serde_json::Value`
            // (unless the target is a direct trait dep).
            if ctx.envelope_routing && call.receiver.is_none() && !ctx.is_trait_target(&call.target) {
                if (ctx.is_struct_target(&call.target) || ctx.is_local(&call.target) || !call.method.is_empty())
                    && !ctx.stub_pkg_crate.contains_key(&call.target)
                {
                    return Some("serde_json::Value".to_string());
                }
            }
            // If calling a trait method, return type is known.
            // Bang only unwraps Result (via `.await?`); Opt/Option is preserved.
            if ctx.is_trait_target(&call.target) {
                let method = if call.method.is_empty() {
                    "call"
                } else {
                    &call.method
                };
                let bare = method.trim_end_matches(['!', '?']);
                // Typed bus: invoke/request of a known message → domain type
                // when that type is in scope for this crate.
                if matches!(bare, "invoke" | "request") {
                    if let Some(msg) = bus_message_name_from_args(&call.args) {
                        if let Some(ret) = ctx.bus_returns.get(&msg) {
                            if bus_return_type_in_scope(ctx, ret) {
                                return Some(ret.clone());
                            }
                        }
                    }
                }
                return ctx.return_type_of(&call.target, method).map(|s| s.to_string());
            }
            // If calling a struct constructor
            if ctx.is_struct_target(&call.target) {
                let method = if call.method.is_empty() { "new" } else { &call.method };
                return ctx.return_type_of(&call.target, method).map(|s| {
                    // Resolve "Self" to the actual struct name
                    if s == "Self" { call.target.clone() } else { s.to_string() }
                });
            }
            // If calling a method on a local (e.g. @dep wear_test_repo typed as trait via name_to_shape)
            if ctx.is_local(&call.target) || ctx.is_trait_target(&call.target) {
                if let Some(t) = ctx.return_type_of(&call.target, &call.method) {
                    return Some(t.to_string());
                }
                // Resolve through the local's inferred type:
                // e.g. `repo` has type `Repository`, so `repo.write_blob(...)` → look up
                // `Repository.write_blob` return type.
                if let Some(local_ty) = ctx.local_type(&call.target) {
                    let method = call.method.trim_end_matches(['!', '?']);
                    if let Some(t) = ctx.return_type_of(local_ty, method) {
                        return Some(t.to_string());
                    }
                }
            }
            // Adapter `@dep` / `@field` used as a bare ident (`routing_table.get_route!`).
            if let Some(fty) = ctx
                .self_field_types
                .get(&call.target)
                .or_else(|| ctx.self_field_types.get(&to_snake(&call.target)))
            {
                let method = call.method.trim_end_matches(['!', '?']);
                let bare_ty = peel_dyn_trait_name(fty).unwrap_or_else(|| fty.clone());
                if let Some(t) = ctx.return_type_of(&bare_ty, method) {
                    return Some(t.to_string());
                }
                if let Some(t) = ctx.return_type_of(fty, method) {
                    return Some(t.to_string());
                }
            }
            // Stub package free functions: `gix.init_bare(path)` → target is "gix",
            // method is "init_bare". Look up (stub_name, method) in method_returns.
            if ctx.stub_pkg_crate.contains_key(&call.target) {
                let method = call.method.trim_end_matches(['!', '?']);
                if let Some(t) = ctx.return_type_of(&call.target, method) {
                    return Some(t.to_string());
                }
            }
            // Also handle receiver-based form: `receiver.method(args)` where receiver is a stub pkg ident.
            if let Some(recv) = &call.receiver {
                if let Expr::Ident(recv_name) = recv.as_ref() {
                    // Receiver is a stub package (e.g. `gix.init_bare(...)`)
                    if ctx.stub_pkg_crate.contains_key(recv_name) {
                        let method = call.method.trim_end_matches(['!', '?']);
                        if let Some(t) = ctx.return_type_of(recv_name, &method) {
                            return Some(t.to_string());
                        }
                    }
                    // Receiver is a local variable with a known type
                    if let Some(local_ty) = ctx.local_type(recv_name) {
                        let method = call.method.trim_end_matches(['!', '?']);
                        if let Some(t) = ctx.return_type_of(local_ty, method) {
                            return Some(t.to_string());
                        }
                    }
                    if let Some(fty) = ctx
                        .self_field_types
                        .get(recv_name)
                        .or_else(|| ctx.self_field_types.get(&to_snake(recv_name)))
                    {
                        let method = call.method.trim_end_matches(['!', '?']);
                        let bare_ty = peel_dyn_trait_name(fty).unwrap_or_else(|| fty.clone());
                        if let Some(t) = ctx.return_type_of(&bare_ty, method) {
                            return Some(t.to_string());
                        }
                    }
                }
                // Receiver is a chained call (e.g. `ThreadSafeRepository.open(path).to_thread_local()`)
                // Recursively infer the receiver's type, then look up the method on that type.
                if let Some(recv_type) = infer_expr_type(recv, ctx) {
                    let method = call.method.trim_end_matches(['!', '?']);
                    // "Self" return means same type as receiver
                    if let Some(t) = ctx.return_type_of(&recv_type, method) {
                        if t == "Self" {
                            return Some(recv_type);
                        }
                        return Some(t.to_string());
                    }
                }
            }
            None
        }
        // Empty list `[]` — element unknown until append
        Expr::ArrayLit(items) if items.is_empty() => Some("Vec<()>".to_string()),
        Expr::ArrayLit(items) => items
            .first()
            .and_then(|e| infer_expr_type(e, ctx))
            .map(|t| format!("Vec<{t}>")),
        Expr::BinaryOp(bin) if matches!(bin.op, BinOp::Add) => {
            // options + [x] → keep/upgrade Vec type
            let left = infer_expr_type(&bin.left, ctx);
            let right = infer_expr_type(&bin.right, ctx);
            match (left.as_deref(), right.as_deref()) {
                (Some("Vec<()>"), Some(r)) if r.starts_with("Vec<") => right,
                (Some(l), _) if l.starts_with("Vec<") && l != "Vec<()>" => left,
                (_, Some(r)) if r.starts_with("Vec<") => right,
                (Some(l), _) if rust_ty_is_stringish(l) => Some("String".into()),
                (_, Some(r)) if rust_ty_is_stringish(r) => Some("String".into()),
                _ if matches!(&*bin.left, Expr::StringLit(_))
                    || matches!(&*bin.right, Expr::StringLit(_)) =>
                {
                    Some("String".into())
                }
                _ => left.or(right),
            }
        }
        Expr::StructLit(name, _) => Some(name.clone()),
        Expr::Ident(name) => ctx.local_type(name).map(|s| s.to_string()),
        Expr::FieldAccess(base, field) => {
            if let Expr::Ident(n) = base.as_ref() {
                if n == "self" {
                    if let Some(ty) = ctx
                        .self_field_types
                        .get(field)
                        .or_else(|| ctx.self_field_types.get(&to_snake(field)))
                    {
                        return Some(ty.clone());
                    }
                }
                if let Some(base_ty) = ctx.local_type(n) {
                    let leaf = lang_type_leaf(base_ty);
                    if let Some(ft) = ctx
                        .field_type(base_ty, field)
                        .or_else(|| ctx.field_type(base_ty, &to_snake(field)))
                        .or_else(|| ctx.field_type(leaf, field))
                        .or_else(|| ctx.field_type(leaf, &to_snake(field)))
                    {
                        return Some(ft.to_string());
                    }
                }
            }
            None
        }
        Expr::IntLit(_) => Some("i64".to_string()),
        Expr::FloatLit(_) => Some("f64".to_string()),
        Expr::BoolLit(_) => Some("bool".to_string()),
        Expr::StringLit(_) => Some("String".to_string()),
        // Layer actions (invoke, request, etc.) return serde_json::Value
        Expr::Action(_) => Some("serde_json::Value".to_string()),
        Expr::Require(inner) => infer_expr_type(inner, ctx).map(|t| {
            peel_option_rust(&t)
                .map(|s| s.to_string())
                .unwrap_or(t)
        }),
        _ => None,
    }
}

/// Message type name from a desugared bus call arg (`invoke Reconcile{…}`).
fn bus_message_name_from_args(args: &[Expr]) -> Option<String> {
    match args.first() {
        Some(Expr::StructLit(name, _)) => Some(name.clone()),
        Some(Expr::Ident(name)) => Some(name.clone()),
        _ => None,
    }
}

/// Whether `ret` can be written as a bare path in this crate (local domain type
/// or language primitive). Foreign domain types are left as `serde_json::Value`.
fn bus_return_type_in_scope(ctx: &GenCtx, ret: &str) -> bool {
    let ret = ret.trim();
    if ret.is_empty() || ret == "()" || ret == "serde_json::Value" || ret.starts_with("Result<") {
        return false;
    }
    if matches!(
        ret,
        "String" | "bool" | "i64" | "i32" | "f64" | "f32" | "Uuid" | "usize"
    ) {
        return true;
    }
    if let Some(inner) = ret
        .strip_prefix("Vec<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return bus_return_type_in_scope(ctx, inner.trim());
    }
    if let Some(inner) = ret
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return bus_return_type_in_scope(ctx, inner.trim());
    }
    // Only domain types defined in *this* crate (set by application codegen).
    ctx.local_domain_types.contains(ret)
}

/// Collect all trait-shaped construct names referenced in flow step bodies.
/// Returns the set of port names that need to be in the Deps struct.
pub fn collect_deps(steps: &[FlowStep], ctx: &GenCtx) -> HashSet<String> {
    let mut deps = HashSet::new();
    for step in steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                collect_deps_from_expr(expr, ctx, &mut deps);
            }
        }
    }
    deps
}

fn collect_deps_from_expr(expr: &Expr, ctx: &GenCtx, deps: &mut HashSet<String>) {
    match expr {
        Expr::Call(call) => {
            if ctx.is_trait_target(&call.target) {
                deps.insert(call.target.clone());
            } else if call.method.ends_with('!') && !call.target.is_empty() {
                // VEIL convention: method! marks port/repo calls. Find matching trait.
                for (name, shape) in &ctx.name_to_shape {
                    if *shape == Shape::Trait {
                        let trait_snake = to_snake(name);
                        // Require exact match or underscore-boundary suffix match
                        // (e.g. "registry" matches "registry" or "acp_session_registry"
                        //  with suffix "_registry", but NOT "extension_registry" matching
                        //  bare "registry" — that's handled by explicit @dep annotations)
                        if trait_snake == call.target
                            || trait_snake.ends_with(&format!("_{}", call.target))
                        {
                            deps.insert(name.clone());
                            break;
                        }
                    }
                }
            }
            if let Some(recv) = &call.receiver {
                collect_deps_from_expr(recv, ctx, deps);
            }
            for arg in &call.args {
                collect_deps_from_expr(arg, ctx, deps);
            }
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) => collect_deps_from_expr(rhs, ctx, deps),
        Expr::Action(a) => {
            for arg in &a.args {
                collect_deps_from_expr(arg, ctx, deps);
            }
            for (_, v) in &a.named_args {
                collect_deps_from_expr(v, ctx, deps);
            }
            if let Some(c) = &a.condition {
                collect_deps_from_expr(c, ctx, deps);
            }
            for e in &a.body {
                collect_deps_from_expr(e, ctx, deps);
            }
            // requires_dep / port targets count as deps
            if let Some(spec) = ctx.statement_specs.get(&a.keyword) {
                if let Some(dep) = &spec.requires_dep {
                    deps.insert(dep.clone());
                } else if let Some(port) = &spec.port_target {
                    deps.insert(port.clone());
                }
            }
        }
        Expr::StructLit(_, fields) => {
            for (_, v) in fields {
                collect_deps_from_expr(v, ctx, deps);
            }
        }
        Expr::Match(scrutinee, arms) => {
            collect_deps_from_expr(scrutinee, ctx, deps);
            for arm in arms {
                for expr in &arm.body {
                    collect_deps_from_expr(expr, ctx, deps);
                }
            }
        }
        Expr::IfExpr(data) => {
            collect_deps_from_expr(&data.condition, ctx, deps);
            for expr in &data.then_body {
                collect_deps_from_expr(expr, ctx, deps);
            }
            if let Some(eb) = &data.else_body {
                for expr in eb {
                    collect_deps_from_expr(expr, ctx, deps);
                }
            }
        }
        Expr::ForLoop { iterable, body, .. } => {
            collect_deps_from_expr(iterable, ctx, deps);
            for expr in body {
                collect_deps_from_expr(expr, ctx, deps);
            }
        }
        Expr::WhileLoop { condition, body } => {
            collect_deps_from_expr(condition, ctx, deps);
            for expr in body {
                collect_deps_from_expr(expr, ctx, deps);
            }
        }
        Expr::Return(inner) => {
            collect_deps_from_expr(inner, ctx, deps);
        }
        _ => {}
    }
}

/// Generate the Deps struct source for a set of trait dependencies.
pub fn gen_deps_struct(dep_names: &HashSet<String>) -> String {
    if dep_names.is_empty() {
        return String::new();
    }
    let mut out = String::from("/// Injected dependencies (ports).\npub struct Deps {\n");
    let mut sorted: Vec<&String> = dep_names.iter().collect();
    sorted.sort();
    for name in sorted {
        out.push_str(&format!(
            "    pub {}: std::sync::Arc<dyn {} + Send + Sync>,\n",
            to_snake(name), name
        ));
    }
    out.push_str("}\n\n");
    out
}

fn binop_to_rust(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::LtEq => "<=",
        BinOp::GtEq => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn unaryop_to_rust(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "!",
        UnaryOp::Neg => "-",
    }
}
